# CLAUDE.md: Orbyn Forge

Guidance for AI coding agents and human contributors working in this repository.
More specific rules live next to the code: [`src/ui/CLAUDE.md`](src/ui/CLAUDE.md),
[`docs/CLAUDE.md`](docs/CLAUDE.md) and [`website/CLAUDE.md`](website/CLAUDE.md).

## Project

Orbyn Forge is an open-source (Apache-2.0) Rust orchestration layer for
Windows and Linux fleets. It drives existing engines (Docker, K3s, Caddy,
PowerDNS, guacd, databases) and never reimplements them.

- **Done:** Phase 1, the core foundation. It covers identity, Cedar
  authorization, agents, durable jobs, audit, plugins and the console.
- **Read first:** `ARCHITECTURE.md`, then `DECISIONS.md`. The product plan is
  `plans/inital_plan.md`.
- **Repository:** `github.com/mr-aybee/forge` holds the source, releases and
  issues. Contributors fork it and open PRs into `develop` (see `CONTRIBUTING.md`).

## Layout

All source code lives under `src/`. New crates go in the matching
`src/<layer>/`; the workspace globs pick them up. Never recreate a top-level
`crates/`, `tools/`, `ui/`, `plugins/` or `agents/`.

| Path | Contents |
| --- | --- |
| `src/domain/forge-core` | Types, typed ids, access vocabulary, **ports** (traits). No I/O. |
| `src/services/*` | `forge-authz` (Cedar), `forge-audit`, `forge-identity`, `forge-jobs`, `forge-pki`, `forge-secrets`, `forge-plugin-host`, … |
| `src/adapters/forge-store` | sqlx repositories and migrations (SQLite default, `postgres` feature) |
| `src/protocol/*` | `forge-proto` (agent wire protocol), `forge-sdk` (WIT, manifest, signing, release feed) |
| `src/apps/*` | `forge-server` (composition root: `src/state.rs`), `forge-agent` (+ helper), `forge-cli`, `forge-plugin-runner`, `forge-analytics-worker`, `forge-file-gateway` |
| `src/tools/fleet-sim` | Fleet benchmark |
| `src/plugins/uptime-probe` | Sample signed plugin (component WAT) |
| `src/ui` | Web console: React, TypeScript, Vite |
| `website/` | Project website (React, TS, nginx Dockerfile) |
| `docs/` | Astro Starlight docs, served at `/docs/` |
| `scripts/` | Packaging (`packaging/`), CI helpers (`ci/`), developer tools (`dev/`), test lab (`setup_testing/`) |
| `.claude/agents/` | Subagent definitions for this repo |

## Commands

```sh
# Gate: run all of these before you push
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings   # alias: cargo lint
cargo test --workspace                                  # includes kill-and-resume tests
cargo deny check && cargo audit
docker run --rm -v "$PWD":/repo zricethezav/gitleaks:v8.21.2 dir /repo --no-banner   # secret scan (.gitleaks.toml)
(cd src/ui && npm ci && npm run build)
(cd docs && npm ci && npm run build)                    # must have no broken links
(cd website && npm ci && npm run build)

# While iterating
cargo ck                          # check, no linking
cargo check -p <crate> && cargo test -p <crate>

# Builds
cargo build --profile perf -p forge-server             # optimised local build
cargo build --release -p forge-server -p forge-agent -p forge-plugin-runner -p forge-analytics-worker -p fleet-sim
target/release/fleet-sim --agents 50 --agents 1000     # updates BENCHMARKS.md
docker build -f website/Dockerfile -t forge-website .
```

### Build tips

- `--release` (fat LTO, one codegen unit) is for shipping. Use `perf` locally.
- sccache: set `[build] rustc-wrapper = "sccache"` in your user-level
  `~/.cargo/config.toml`, **never** in the repo's `.cargo/config.toml`, where it
  breaks CI and `cross`. `python3 scripts/dev/build_env.py` reports your setup.
- Reclaim disk with `python3 scripts/dev/build_env.py --sweep` before `cargo clean`.
- On Windows, build on the Windows filesystem, not through WSL on `/mnt/<drive>`.
  `.cargo/config.toml` links with `rust-lld.exe`. If a link error looks
  linker-specific, set `CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER=link.exe`,
  as the release workflow does.
- Developer scripts are Python 3 using only the standard library. `install.ps1`
  and the catalog recipes stay PowerShell because they run on Windows nodes.

## Architecture rules (enforced in review)

1. **Dependencies point inward.** Services depend on `forge-core` ports, never
   on `forge-store` or other adapters. Concrete types are wired only in
   `src/apps/forge-server/src/state.rs`.
2. **Authorize where it takes effect:** in the API handler (`Caller::authorize`),
   the job runner (every step), the agent hub, the agent and the plugin host.
   Never rely on the UI.
3. **Audit every mutation**, including denials, with `Caller::audit` or `AuditRecorder`.
4. **Extend, don't modify.**
   - New job kinds implement `JobHandler`.
   - New actions go into `forge_core::Action` *and* the Cedar schema.
   - New host functions go into the WIT, `forge_sdk::host_functions`, the
     runner linker and `ForgeHostFunctions`.
5. **Portable SQL.**
   - Use `TEXT`/`BIGINT` only, `$n` placeholders, timestamps in epoch ms and
     flags as 0/1.
   - `0001_initial.sql` is the baseline. Add every change as a new migration
     after it, and never edit a released one.
6. **Steps are idempotent.** A job step may run twice after a crash.
7. **No Phase 2+ features** (browser access, deployments, proxies, DNS,
   containers) unless asked. Design the extension points only.

## Security rules

- No secrets in URLs, logs or the browser. Request logs record paths only.
- Store only hashes of tokens. Compare secrets with `forge_secrets::crypto::constant_time_eq`.
- Use one crypto provider, `ring`. **No OpenSSL.**
- `unsafe` is denied workspace-wide. The Windows SCM module is the documented exception.
- No `unwrap()`/`expect()` outside tests.
- Never commit real hostnames, IPs, usernames or keys. Lab machines are
  `lab-a` … `lab-z`. Examples use placeholders (`<node-name>`) or RFC 5737
  addresses (`192.0.2.x`).
- Report vulnerabilities privately (see `SECURITY.md`), never in a public issue.

## Rust code quality

These rules apply to new and changed code. Known debt is listed in
`plans/code-quality.md`. Do not add to it, and do not start a repo-wide clean-up
unless asked.

- **Thin handlers.**
  - An API handler parses, authorizes, calls, audits and responds.
  - Business rules, multi-step writes and reconcilers go in a service type that
    takes ports (`Arc<dyn Port>`) and returns `DomainResult`. `forge-files` is
    the model.
  - New features add no `state.store` calls in handlers and no
    `forge_store::Store` outside `state.rs`.
  - Put no background loops under `api/`. Nothing outside `api/` imports `api/`.
- **Types, not strings.**
  - Use serde enums for actions, ops, statuses, runtimes and OS names, and typed
    structs for specs.
  - Add no new `Json<Value>` or `json!` bodies where a struct fits.
  - Don't serialise-then-patch.
  - Build node commands from typed values.
- **Typed errors.**
  - Add no `Result<_, String>` or `map_err(|e| e.to_string())` in non-trivial code.
  - Don't signal conditions through string prefixes.
- **Never hide a failure.**
  - Don't end a store or decode call with `.unwrap_or_default()` or `.ok()`
    unless a comment says why the default is correct.
  - Handlers propagate with `?`. Background loops log and skip the tick.
- **One definition.**
  - Search before writing a helper (`EntityRef` constructors, `write_private`,
    `sha256_hex`, URL encoding, name validation).
  - Audit action names, error codes, job kinds and settings keys are constants
    defined once.
- **No process-global state.** Per-server state is a field on `Services`.
- **Time comes from the `Clock` port**, never the system clock.
- **No blocking in async.** Run `std::fs` and `std::process` calls in
  `spawn_blocking`, or use the `tokio` equivalents.
- **Lists are bounded and batched.** List endpoints take a clamped limit. Use
  `can_many` and batch store methods.
- **Multi-step writes are atomic or compensate explicitly.** A comment says which.
- **Small functions and parameter structs.**
  - No `#[allow(clippy::too_many_arguments)]`.
  - Avoid two or more `bool` parameters.
- **Every `#[allow(..)]` has a reason comment.**

## Dependencies

- Only MIT, Apache-2.0, BSD and the permissive equivalents in `deny.toml` are
  allowed.
- **Ask a maintainer first** before adding a dependency that:
  - has another license;
  - adds more than 5 MB to the agent;
  - pulls in OpenSSL.

  Record each such decision in `DECISIONS.md`.
- Keep the tree small:
  - Reuse what is already in `Cargo.lock`.
  - Turn off unused default features.
  - Never add a second major version of a crate (`cargo tree -d` lists them).

## Workflow

- Fork `mr-aybee/forge`, clone your fork, and branch from `upstream/develop`.
  Open a PR into `develop`. `feat/…` and `fix/…` PRs are squash-merged into
  `develop`; `develop` is merged into `master` with a merge commit, never
  squashed, and `master` is then merged back into `develop` the same way.
  Releases are tagged on `master`.
- Work in that one clone. Don't create git worktrees or sibling copies.
- Make small commits with imperative subjects.
- Keep `DECISIONS.md` current for significant choices.
- Tests poll with deadlines, never fixed sleeps. Use real SQLite in `tempfile` dirs.
- A change is not done until the docs describe it (see `docs/CLAUDE.md`). Also
  keep these current:
  - `README.md`: feature table, quick start, layout, benchmark figures.
  - `ARCHITECTURE.md` and `branding/forge-architecture.svg` when components,
    trust boundaries or flows change.
  - `docs/src/content/docs/reference/rest-api.md` for every endpoint change.

## Subagents

Definitions live in `.claude/agents/` and are loaded automatically:
- `rust-architect` and `system-architect` for design;
- `security-reviewer`;
- `cedar-policy-author`;
- `test-engineer`;
- `ui-engineer`;
- `docs-writer`;
- `ci-engineer`;
- `dependency-auditor`;
- `lab-operator`.

Add new ones there.
