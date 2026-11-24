---
title: Contributing
description: How to send a change to Forge from your fork, and the toolchain, checks and conventions it must pass.
sidebar:
  order: 1
---

This page shows you how to propose a change to Forge through a pull request,
and which checks and conventions the change must pass. The source is public at
[`orbyntech/forge`](https://github.com/orbyntech/forge) under the Apache
License 2.0.

## Send a change

You can't push to `orbyntech/forge` directly. Work in your own fork and open
a pull request into `master`.

1. Fork `orbyntech/forge` on GitHub (**Fork** on the repository page).
2. Clone your fork and add the main repository as `upstream`:

   ```sh
   git clone https://github.com/<your-user>/forge.git
   cd forge
   git remote add upstream https://github.com/orbyntech/forge.git
   ```

3. Create a branch from the latest `upstream/master`:

   ```sh
   git fetch upstream
   git switch -c <branch-name> upstream/master
   ```

4. Make your change, with its docs (see `CLAUDE.md`, **Documentation**).
5. Run the checks:

   ```sh
   cargo fmt --all
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace
   cargo deny check
   (cd src/ui && npm ci && npm run build)
   (cd docs && npm ci && npm run build)
   ```

6. Push the branch to your fork:

   ```sh
   git push -u origin <branch-name>
   ```

7. On GitHub, open a pull request from your branch into
   `orbyntech/forge:master`.

CI runs on the pull request. The first time you contribute, a maintainer
must approve the workflow run before it starts. A maintainer reviews the
change and merges it once CI is green.

:::caution
Report security issues privately, as `SECURITY.md` describes, through a
GitHub Security Advisory. Don't open a public issue or pull request for them.
:::

## Toolchain

- Rust stable (MSRV 1.88, edition 2024), `clippy`, `rustfmt`
- Node 20+ for `src/ui` and `docs`
- `cargo install cargo-deny cargo-audit`

## Everyday commands

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check && cargo audit
(cd src/ui && npm run build && npm audit --omit=dev)
(cd docs && npm run build && npm audit --omit=dev)
```

CI (`.github/workflows/ci.yml`) runs these on Linux, and clippy, build and
tests on Windows when the agent changes. The static musl release builds run
only in the release workflow (see [CI and releases](/docs/development/releases/)).

The docs build fails on a link to a page or heading that does not exist
(`starlight-links-validator`). It also rejects links to `localhost`: write a
local address as code, for example `http://localhost:8080`.

The `docs_coverage` tests in `forge-server`, `forge-agent`, `forge-agent-helper`
and `fleet-sim`, and `installer_docs.rs`, fail when a command or flag is
missing from its page under `reference/cli/`. Document a new command or flag in
the same change.

## Faster builds on Windows

Build on the Windows filesystem. A WSL build of a checkout under `/mnt/` is
slow because every file access crosses the filesystem boundary.

| Setting | Where | Effect |
| --- | --- | --- |
| LLD linker (`rust-lld.exe`) | `.cargo/config.toml`, applies to everyone on Windows | Shorter link step. LLD ships with the Rust toolchain. |
| sccache | User-level `~/.cargo/config.toml`, once per machine | Compiled dependencies are cached outside `target/`, so they survive a `cargo clean` and are shared by every Rust project on the machine. |
| `cargo ck` | Alias for `cargo check --workspace --all-targets` | Type-checks without code generation or linking. |
| `cargo lint` | Alias for `cargo clippy --workspace --all-targets -- -D warnings` | The lint gate CI runs. |
| `--profile perf` | `Cargo.toml` | Optimised build with thin LTO. Use it instead of `--release` for local performance work. |

Turn on sccache once per machine:

1. Install it: `cargo install sccache --locked`.
2. Add it to your user-level Cargo config, `~/.cargo/config.toml` on Linux or
   `%USERPROFILE%\.cargo\config.toml` on Windows:

   ```toml
   [build]
   rustc-wrapper = "sccache"
   ```

3. Check the setup and the cache statistics:

   ```sh
   python3 scripts/dev/build_env.py
   ```

The cache lives in sccache's default directory (`~/.cache/sccache` on Linux,
`%LOCALAPPDATA%\Mozilla\sccache` on Windows); set `SCCACHE_DIR` or
`~/.config/sccache/config` to move it or change its size. To turn sccache on
for one terminal only, run
`eval "$(python3 scripts/dev/build_env.py --shell sh)"` (bash) or
`python scripts/dev/build_env.py --shell pwsh | Out-String | Invoke-Expression`
(PowerShell).

:::caution
Never put `rustc-wrapper` in the repository's `.cargo/config.toml`. It breaks
CI and `cross` builds, which do not have sccache.
:::

To free disk space, remove build artifacts that no build has used for 14 days
(needs `cargo install cargo-sweep --locked`):

```sh
python3 scripts/dev/build_env.py --sweep
python3 scripts/dev/build_env.py --sweep --days 7
```

The release workflow links Windows binaries with the MSVC linker by setting
`CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER=link.exe`. Set the same variable
locally if you suspect a link error comes from LLD.

:::tip
Exclude the repository's `target` directories, `%USERPROFILE%\.cargo` and the
sccache directory from real-time antivirus scanning. Scanning every build
artifact slows compilation noticeably.
:::

## Code conventions

- **Layers:** put code in the right `src/` layer; services depend on ports in
  `forge-core`, never on adapters. Wire concrete types only in
  `forge-server/src/state.rs`.
- **Small traits:** a port has one purpose (`JobRepository`, `AuditRecorder`).
  Add a new trait rather than widening one.
- **Open for extension:** new job kinds, host functions and roles are
  additions (register a handler, add a schema action), not edits to engines.
- **Errors:** return `DomainError` from ports; map to HTTP only in
  `forge-server/src/error.rs`. Never leak internal error text to clients.
- **No `unwrap` in non-test code** (`clippy::unwrap_used` warns). `unsafe` is
  denied workspace-wide; the Windows SCM module is the single, commented
  exception.
- **Security:** authorize at the point of effect, audit every mutation, never
  log secrets or query strings, compare secrets in constant time.
- **Comments** explain *why*; names explain *what*.

## Dependency policy

Only MIT, Apache-2.0, BSD and equivalent permissive licenses (`deny.toml`).
**Ask before adding** a dependency with another license, one that adds more
than 5 MB to the agent, or anything that brings in OpenSSL. Record the
decision in `DECISIONS.md`.

## Commits

Make small, focused commits with an imperative subject line. Update
`DECISIONS.md` when you make or change an architectural decision, and the docs
when behaviour changes.
