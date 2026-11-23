# Code quality plan: smells, patterns and refactoring work

This file is a work list for whoever picks up code-quality work on Orbyn Forge,
human or agent. It records what a three-part review found (architecture, Rust
code smells, the React UI), with evidence, a fix for each item and a way to
tell when the item is done. You do not need any other context to start, apart
from `CLAUDE.md`, `ARCHITECTURE.md` and `DECISIONS.md`.

Security issues are not tracked here. Report them privately as `SECURITY.md`
describes, through GitHub Security Advisories. Some entries name an earlier
security finding ID (for example S1) for context only.

## How to use this file

1. Pick one item. Each has an ID: `A` for architecture, `R` for Rust code,
   `U` for the UI. Work on one ID per branch and pull request.
2. Read the cited code first. Every count and line number below was correct
   when the review ran, and the code has moved since. **Re-check the evidence
   before you change anything**, and re-count with the search given in the
   entry.
3. Check the **Confidence** line. "Read" means a reviewer opened the code and
   traced it. "Counted" means a search matched and nobody inspected each hit.
   Counted items need a first pass to separate real cases from false ones.
4. Follow the repository rules in `CLAUDE.md`: branch from `origin/develop`,
   small commits, docs in the same change, new migration files only, no new
   dependency without asking.
5. Refactors must not change behaviour. Keep the wire format, the REST API,
   the database schema and the CLI output the same unless the entry says
   otherwise. If a change is visible to users, update `docs/`.
6. Verify with the narrowest command that covers the change
   (`cargo check -p <crate>`, `cargo test -p <crate>`, `cargo lint`), then the
   workspace commands before pushing. For UI work run `npm run build` in
   `src/ui`.
7. When an item is finished, mark it in the status table at the end of this
   file in the same pull request.

## How the review was done, and its limits

- Read-only. No build, no `cargo clippy`, no test run. The one exception:
  `npx tsc --noEmit` on the UI, which passed with no diagnostics.
- Size at review time: 345 Rust files and about 111,000 lines under `src/`;
  152 UI files and about 39,300 lines under `src/ui/src`.
- About 10 of the 55 API handler files were read in depth. The rest were
  judged by counts. Most of the 20 largest Rust files were not read in full.
- Not analysed: `.clone()` in hot paths, locks held across `await`, deep
  nesting, public items that could be private, circular imports in the UI, a
  field-by-field comparison of the UI types with the OpenAPI document.
- Counts labelled "non-test" still include inline `#[cfg(test)]` modules, so
  they overstate production use.

## Summary

The crate-level layering is clean and enforced by the compiler. The problems
sit one level down:

| Theme | Where | Items |
| --- | --- | --- |
| Business logic in HTTP handlers, concrete store exposed | `forge-server` | A1 |
| Strings and raw JSON doing the work of enums and structs | server, agent, CLI, UI | A2, A4, R1, R2, R3, R10, U5 |
| Failures hidden as empty results | server, UI | R4, U6 |
| Duplicated helpers | server, agent, CLI, UI | R6, U9 |
| Missing tooling and tests | UI, lint config, CI | R8, R9, A8, U1, U2 |
| Oversized functions and components | server, CLI, UI | R5, U4 |

Suggested order:

1. **Cheap and high value:** U1, R8, R4, R6 (the `EntityRef` constructors),
   U7 (socket leak and fetch races), R11.
2. **Medium:** A2, R2, U5, U2, U3, A3, A5, A6.
3. **Large, one feature at a time:** A1, R1, R3.

---

# Part A: architecture and patterns

## A1. Business logic lives in HTTP handlers; the concrete store is exposed

**Severity:** High. **Size:** large, done feature by feature.
**Confidence:** read for `storage.rs`, `clusters.rs`, `routing.rs`,
`sites.rs`, `settings.rs`; counted for the rest.

### Problem

`CLAUDE.md` rule 1 says services depend on ports in `forge-core` and concrete
types are wired only in `src/apps/forge-server/src/state.rs`. The service
crates follow it. The server crate does not:

- `src/apps/forge-server/src/api/` is about 22,000 lines in 55 files. All
  eleven service crates together are about 11,400 lines.
- `state.rs:37` declares `pub store: Store`, the concrete adapter type.
  `Services` has 35 public fields.
- `state.store` appears 486 times in the server, 414 of them under `api/`, in
  46 of the 55 handler files. Worst: `api/stacks.rs` (30), `api/storage.rs`
  (27), `api/fabric.rs` (27).
- The concrete type is named outside `state.rs`: `src/alerting.rs:31` holds
  `Arc<forge_store::Store>` and `src/access.rs:39` takes
  `&forge_store::Store`.
- Background loops live under `api/` and are started from `src/lib.rs:113-121`
  (`api::fabric_reconcile::run`, `api::ingress_dns::run`,
  `api::cert_renewal::run`, `api::package_updates::run`,
  `api::system_update::run`, `api::agent_updates::auto_run`).
- Coupling runs the wrong way: `jobs/node_bootstrap.rs` calls
  `crate::api::onboarding` and `crate::api::logins`;
  `agent_hub/handlers.rs` calls `crate::api::fabric`, `fabric_reconcile` and
  `logins`; `state.rs:47-53` takes four types from `api::*`.
- Handler modules call each other 78 times through `super::<module>::`.
  `sites::run_as`, `sites::run_on_node`, `ingress::load_profile` and
  `routing::Resolver` act as shared services.

### Example

`install` in `api/storage.rs:425` runs to about line 635. It validates the
topology, applies an OS and architecture matrix, checks drive counts, checks
port and path collisions, mints a credential, writes a vault item, the store,
the members and N jobs, audits and builds the response. Lines 574-594 make
those writes in sequence with no transaction, so a failure midway leaves an
orphaned vault credential or a store with no jobs.

`assign` in `api/clusters.rs:187-260` makes a multi-step change with a manual
compensating write (`:220-226`) and a `tokio::spawn` (`:236`).
`api/clusters.rs:169-171` deletes a profile, a fabric and a cluster as three
separate writes.

None of this can be tested without starting a full server.

### Fix

Do this in three steps, one feature at a time. Start with storage, then
stacks, then fabric.

1. **Move code out of `api/` (small).** Create a feature module per area
   (`fabric/`, `ingress/`, `storage/`, `sites/`) with `http.rs` for the
   handlers and separate files for logic and reconcilers. `api/` may import
   the feature modules. Nothing outside `api/` may import `api/`.
2. **Extract a use-case type (medium).** For each feature, write a struct that
   takes `Arc<dyn Port>` values and returns `DomainResult`. Handlers shrink to
   parse, authorize, call, audit, respond. `forge-files::FileService`
   (`src/services/forge-files`) is the model already in the repository,
   including its in-memory fakes in `src/services/forge-files/src/tests.rs`.
3. **Promote and close (large).** Move the clean use-case modules to
   `src/services/*` crates and make `store` private on `Services`.

Record the decision in `DECISIONS.md`: either amend D-001 to allow thin CRUD
handlers that call a port directly, or commit to full extraction.

### Done when

- For the feature you moved: its handler file contains no `state.store` call,
  and the use-case type has unit tests that run without the server harness.
- `grep -rn "crate::api::" src/apps/forge-server/src --include=*.rs` finds no
  hit outside `api/` for that feature.
- Behaviour, REST responses and audit events are unchanged (the existing
  integration tests pass).

---

## A2. `agent.command` is an untyped JSON pipeline that every node feature edits

**Severity:** High. **Size:** medium. **Confidence:** read.

### Problem

`CLAUDE.md` rule 4 says new features are added, not edited into engines. Job
kinds obey it. Node commands do not. In
`src/apps/forge-server/src/jobs/agent_command.rs`:

- `parse_command` (`:43-119`) matches on `"type"` strings to build the wire
  command.
- `required_action` (`:245-281`) decides the Cedar action separately, through
  JSON pointers into the same payload. The two functions must agree by hand.
- Per-kind special cases sit in the handler: certificate attachment at
  `:225-228`; timeouts that depend on `"/command/op" == "pull"` or
  `"/command/action" == "apply"` at `:317-319`.
- Fifteen server files build commands with `json!({ "type": … })`, for
  example `api/clusters.rs:237`.
- `:307-308` re-dispatches after a restart because "commands are idempotent
  reads", but the same handler carries container remove and pull, site apply
  and fabric changes. Whether each is idempotent on the node was not verified.

### Example

To add one node command today you edit: the `prost` oneof
(`src/protocol/forge-proto/src/messages.rs:257`), `parse_command`,
`required_action`, the agent's long-running classification
(`src/apps/forge-agent/src/agent.rs:179-189`), the executor match
(`src/apps/forge-agent/src/executor.rs:44`) and the helper allow-list. If
`parse_command` and `required_action` read the payload differently, the
command is authorized as one thing and executed as another.

### Fix

1. Define one serde-tagged `NodeCommand` enum in `forge-catalog`, which
   already owns `FabricOp`, `IngressOp` and `SiteSpec`.
2. Give it methods: `required_action()`, `wait_budget()`, `is_idempotent()`,
   `into_proto()`.
3. Callers construct the enum. The job stores its serialised form, so stored
   payloads keep the same JSON shape (check with a round-trip test against
   existing payloads).
4. Keep the fail-closed default (`_ => Action::ServerCommand`).
5. Add a test that every variant has an explicit action.
6. Only re-dispatch after a restart when `is_idempotent()` is true.

Ask the `system-architect` or `rust-architect` subagent to review the design
before coding; this touches the control plane, the wire protocol and the
agent.

### Done when

- No `json!({ "type":` command construction remains in the server.
- `parse_command` and `required_action` are replaced by methods on one type.
- Adding a command means adding one enum variant plus its executor arm.

---

## A3. Process-global mutable state outside `AppState`

**Severity:** Medium. **Size:** small per item. **Confidence:** read.

### Problem

Function-local `static OnceLock<Mutex<…>>` registries hold per-server state.
The integration tests start many servers in one process
(`src/apps/forge-server/tests/it/main.rs`), so those servers share it. The
state is invisible in the composition root and is never reset.

Sites, all under `src/apps/forge-server/src/`:

- `facts.rs:25-46`
- `measures.rs:26`
- `names.rs:388-396`
- `api/fabric_reconcile.rs:94-105`
- `api/onboarding.rs:57-60`
- `api/passkeys.rs:185`
- `api/storage.rs:75` and `:136-137`

### Example

Two test servers in one binary both write node facts into the map in
`facts.rs`. A test can read facts another test's server recorded.

### Fix

Move each registry into a field on `Services`, as `telemetry`, `tunnels` and
`login_throttle` already are. One pull request per registry is fine.

### Done when

`grep -rn "static .*OnceLock" src/apps/forge-server/src` shows no per-server
state (constants and compiled regexes may stay).

---

## A4. Stringly-typed domain model and error signalling

**Severity:** Medium. **Size:** small per item. **Confidence:** read.
See also R2 for the wire and request side.

### Problem

- **Raw JSON in the model.** `src/domain/forge-core/src/model.rs` fields at
  `:791` and `:892-922` are `serde_json::Value` (fabric `spec`, `pins`,
  `leases`, `status`; cluster `profile`; site `config`, `edge_status`). Code
  probes them by key: `jobs/agent_command.rs:201-205`
  (`site.config["edges"]`, `/spec/cert_id`), `api/clusters.rs:264-268`.
- **Strings where enums belong.** `status`, `kind`, `runtime`, `topology`,
  `provider`, `mode`, `role` are `String` at `model.rs:340,482,711,789,804,806`,
  in `storage.rs:34-82` and `stacks.rs:95`. `ApprovalRepository::decide_approval`
  takes `status: &str` (`ports.rs:144`).
- **A string protocol inside an error.** The MFA denial is signalled by the
  prefix `"mfa_required:"` (`src/services/forge-authz/src/authorizer.rs:12,89`)
  and parsed back out in `src/apps/forge-server/src/error.rs:52`.
- **A magic prefix on a hash field.** "This session is an API key" is encoded
  as an `apikey:` prefix on `Session.id_hash`
  (`src/apps/forge-server/src/auth.rs:57,171,353,393`).
- **One error type with free text.** `DomainError` carries a `String` in every
  variant; `DomainError::internal(...)` appears about 141 times.

### Example

A typo such as `status: "deployd".into()` compiles and is stored. Renaming the
`mfa_required:` prefix in one crate silently turns the step-up prompt into a
plain 403.

### Fix

- Add `DomainError::StepUpRequired(String)` or a typed reason on `Denied`, and
  drop the prefix parsing.
- Add `SessionOrigin { Web, Bearer, ApiKey { id }, Connector }` on `Caller`.
- Turn status-like strings into serde enums with
  `#[serde(rename_all = "snake_case")]`. Stored values stay `TEXT`, so no
  migration is needed. Decide how unknown stored values are handled (error,
  not a silent default).
- Store typed `FabricSpec` and `SiteSpec` in the model instead of `Value`.

### Done when

Each converted field is an enum or struct, the JSON and database
representation is byte-for-byte the same (round-trip test), and the string
matches for it are gone.

---

## A5. Port design gaps

**Severity:** Medium. **Size:** small to medium. **Confidence:** read.

### Problem

`forge-core` has 52 traits and `forge-store` has 43 `impl … for Store`. All
are object-safe and return `DomainResult`. Keep `async_trait` while ports are
`dyn`. The gaps:

- **No read by key.** `SettingsRepository` (`ports.rs:405-409`) has no
  `get_setting`. Seven callers load every setting and scan, for example
  `api/fabric.rs:45`, `names.rs:236`, `bootstrap.rs:70`.
- **No transaction port.** Cross-repository writes in handlers are not atomic
  (see A1). Inside the adapter, transactions are used correctly.
- **Unbounded lists.** Of 28 `list_*` methods in `ports.rs`, one takes a
  limit.
- **Mixed concerns.** `ServerRepository` (`:198-232`) holds node DNS labels;
  `EnrollmentRepository` (`:235-262`) holds certificate revocation;
  `SessionRepository` (`:103-112`) holds OAuth codes; `PermissionRepository`
  (`permissions.rs:146`) holds service accounts.
- **Tuple returns.** `Vec<(ServerId, String, bool)>` at `:231`;
  `Option<(Option<ClusterId>, Option<String>)>` at `:261`.
- **Long parameter list.** `begin_idempotent` (`:171-179`) takes six
  positional parameters.

### Example

Reading the fabric pool setting loads the whole settings table and filters in
Rust, on every call.

### Fix

- Add `get_setting(key)`.
- Add a `Page { limit, before }` parameter to lists that grow with the fleet
  (servers, jobs, installations, applications).
- Replace tuple returns with named structs.
- Split the mixed traits when next edited, by adding a new trait (rule 4), not
  by widening one.
- For atomic multi-repository writes, design a unit-of-work port with the
  `rust-architect` subagent first.

### Done when

The seven `all_settings()`-and-scan callers use `get_setting`; fleet-sized
lists take a limit; no port returns a bare tuple.

---

## A6. Two competing grant models

**Severity:** Medium. **Size:** medium. **Confidence:** read.
**Related security finding:** S1 (an earlier, privately tracked finding).

### Problem

- Legacy role bindings: `Services::grant` and `revoke` (`state.rs:384-393`),
  used from `api/tenancy.rs:297,337` and `bootstrap.rs:49`.
- Access profiles (D-086): `src/access.rs`, guarded by `access_lock`. It
  mirrors each role as a built-in profile (`access.rs:44-66`).
- Both are loaded at start (`state.rs:121-126`).

### Example

"Who can do X?" and "revoke everything for this user" must consult two tables
and two link mechanisms. The security review found that the legacy path skips
a privilege check the newer path enforces.

### Fix

Express a role binding as an assignment of the matching built-in profile,
behind the same REST API. Use a new migration file to convert existing rows.
Add a `DECISIONS.md` entry. Fix S1 first or in the same change.

### Done when

One grant mechanism remains in the code; the role-binding endpoints still
work for existing clients, or are removed with the docs and CLI parity list
updated.

---

## A7. Settings keys and their rules are scattered

**Severity:** Medium. **Size:** small. **Confidence:** read.
**Related security finding:** S23.

### Problem

Setting-key constants are spread across `api/agent_updates.rs:252`,
`api/fabric.rs:34`, `names.rs:220`, `bootstrap.rs:15`, with a computed key in
`api/package_updates.rs:291`. The generic settings editor keeps a reserved
list by hand (`api/settings.rs:38-52`) and a hidden prefix (`:63`). A search
found no `get_setting("…")` or `set_setting("…")` call with a string-literal
key; keys compared after an `all_settings()` read were not checked.

### Example

A new feature adds a `tunnel.` key and nobody adds it to the reserved list, so
the generic editor can overwrite it without validation.

### Fix

One `settings` module with a typed registry: key, owner, reserved, hidden,
parser. The generic editor consults the registry.

### Done when

Every key the server reads is declared in one place, and the editor's reserved
and hidden lists are derived from it.

---

## A8. Inconsistent pagination and handler details

**Severity:** Medium. **Size:** small. **Confidence:** read where cited,
sampled otherwise.

### Problem

- Only audit has a keyset cursor (`api/audit.rs:47-60`). Clamps differ:
  `alerts.rs:274` (500), `audit.rs:146` (200), `jobs.rs:37` (none; security
  finding S34).
- Audit calls are manual: 184 `caller.authorize` calls against 65
  `caller.audit*` and 32 direct audit calls. Completeness cannot be checked
  mechanically.
- `storage.rs:431-452` validates before authorizing at `:453`.
- The `Ws` extractor runs `list_workspaces` on every request (`auth.rs:312`).
- Many handlers return ad hoc `serde_json::Value` (`clusters.rs:52-54`),
  which weakens the typed OpenAPI work (D-077). See R3.

### Fix

- One `Page` query type and one response envelope.
- Cache the workspace id in `Services`.
- Add a test that walks `openapi::ROUTES` and asserts each mutating route
  writes an audit row.
- Authorize before validating in `storage.rs`.

### Done when

List endpoints share one pagination type with one documented maximum
(`docs/src/content/docs/reference/rest-api.md`), and the audit-coverage test
exists and passes.

---

## A9. PostgreSQL is built in CI but never tested

**Severity:** Medium. **Size:** small. **Confidence:** read.

### Problem

`.github/workflows/ci.yml:157-170` only runs `cargo build --features postgres`.
The SQL discipline looks right (migrations use only `TEXT` and `BIGINT`;
PRAGMAs are confined to `src/adapters/forge-store/src/lib.rs:72-75`), but
rule 5 ("portable SQL") is unverified.

### Fix

Run the `forge-store` test suite against a PostgreSQL service container in CI.
Use the `ci-engineer` subagent.

### Done when

CI runs the store tests on both SQLite and PostgreSQL.

---

## A10. Smaller structural items

**Severity:** Low. Fix when the area is next reworked.

- **Crate placement.** `forge-file-gateway` sits under `apps/` but is a
  library linked by the server (`forge-server/Cargo.toml:21`).
  `forge-catalog` (about 9,000 lines of validation and specs) sits under
  `protocol/` but is domain logic. `forge-identity` contains an HTTP adapter
  (`reqwest`). `forge-s3` has no port and is called from `api/storage.rs`.
- **Engine cycle.** `AgentRolloutJob` gets the engine through
  `Arc<OnceLock<Arc<JobEngine>>>` (`state.rs:180,271`). A small `JobSubmitter`
  port removes it.
- **Wide job handlers.** `NodeBootstrapJob` has 16 injected fields, `StackJob`
  11. This is a symptom of A1.
- **Clock bypasses.** `api/onboarding.rs:63`, `jobs/node_bootstrap.rs:121`,
  `src/adapters/forge-ssh/src/ca.rs:72`, `src/adapters/forge-s3/src/lib.rs:160`
  read the system time directly. Elsewhere the server uses `clock.now()` 178
  times. See also R6 (`now_ms`).
- **Large pure module.** `forge-core/src/files.rs` is 1,451 lines of path
  policy. It is pure, so acceptable; it could become a module tree.

---

# Part B: Rust code smells

## R1. Errors as plain strings in the agent and catalog

**Severity:** High. **Size:** large, module by module.
**Confidence:** signatures counted; `apps.rs`, `agent.rs`, `files/mod.rs`
partly read.

### Problem

There are 373 `Result<_, String>` signatures: 259 in `forge-agent`, 43 in the
server, 40 in `forge-catalog`. Also 123 `map_err(|e| e.to_string())`. Worst
files: `forge-agent/src/files/mod.rs` (49), `files/windows.rs` and
`files/linux.rs` (22 each), `apps.rs` (22), `files/manage.rs` (19).

The workspace already has 12 proper error enums (`DomainError`, `FileError`,
`PkiError` and others). The agent has only `FsError` (`files/mod.rs:126`),
used in 6 signatures, with a `From<String>` catch-all.

### Example

A file operation fails on a node. The server cannot tell "not found" from
"permission denied" from "tool missing", so it reports a generic code
(`node_error`, `file_error`, `bad_node_response`) and the operator sees only
the message text.

### Fix

1. Add an `AgentError` enum with `thiserror` (already a dependency) and a few
   kinds: `NotFound`, `Exists`, `Denied`, `Unsupported`,
   `Tool { cmd, stderr }`, `Io`.
2. Carry the kind in `CommandResult` so the server can map it to a specific
   API error. This is a wire change: make the new field optional and keep old
   agents working (agents and servers of different versions must interoperate).
3. Migrate one module at a time, starting with `files/`. Do not do a
   repository-wide style pass.

### Done when

For the module you migrated: no `Result<_, String>` remains, and the server
returns a specific error code for at least not-found and denied.

Count with: `grep -rn "Result<[^>]*, String>" src --include=*.rs | wc -l`.

---

## R2. Actions, ops, statuses and OS names as strings

**Severity:** High. **Size:** medium. **Confidence:** counted, plus
`agent.rs:154-265` and `storage.rs:425-614` read. See A4 for the model side.

### Problem

- 16 `pub action: String` or `pub op: String` fields:
  `src/protocol/forge-proto/src/messages.rs:356,366,385,401,505`,
  `src/domain/forge-core/src/model.rs:335,549,560,637`, and request structs in
  `api/{applications,containers,packages,stacks,storage}.rs`.
- 34 `match action {` or `match op {` blocks on string literals, for example
  `forge-agent/src/apps.rs:510,814`, `forge-server/src/jobs/stack.rs:180,278,301`,
  `jobs/storage.rs:72,223,262`.
- `api/storage.rs:483-494` matches `(server.os.as_str(), req.runtime.as_str())`
  on `"linux"`, `"windows"`, `"native"`, `"docker"`. `InstallRequest` declares
  `provider`, `runtime` and `topology` as `String` with a doc comment listing
  the legal values.
- Status literals: `status: "deployed".into()` 8 times, `"pending"` 5 times,
  `.status == "installed"` 4 times, `.os == "windows"` 4 times.
- Job kinds appear twice: `fn kind() -> "storage.action"` in the handler and
  `kind: "storage.action".into()` at the submit site. Ten kinds;
  `"agent.command"` 4 times, `"ssh.bootstrap"` 3 times.

### Example

`forge-agent/src/agent.rs:177-181` decides whether a command runs in the
background with `c.op == "pull"` and
`matches!(a.action.as_str(), "deploy" | "run" | "follow")`. A new action name
silently takes the blocking path and stalls the agent's command loop.

### Fix

- Serde enums with `#[serde(rename_all = "snake_case")]` for action, op,
  runtime, topology, OS and status. The wire format stays the same. The
  long-running decision becomes an exhaustive `match`.
- For `prost` messages, keep the string on the wire and convert at the
  boundary with a fallible parse.
- Add `const KIND: &str` to each job handler and use it at the submit site.

### Done when

The 34 string matches are exhaustive enum matches, and each job kind literal
appears once.

---

## R3. Untyped JSON as the working data model

**Severity:** High. **Size:** large, endpoint group by endpoint group.
**Confidence:** counted, plus `filescmd.rs` and `service_accounts.rs` read.

### Problem

- **Server.** 156 handler signatures return `Json<Value>` (or a `Vec` of it)
  against about 92 typed ones, out of 343 public handlers. Responses are built
  with `json!`, so the OpenAPI schema cannot describe them and a renamed field
  still compiles. `json!(` appears 1,307 times in non-test files.
- **Serialise then patch.** A recurring pattern:
  `let mut v = serde_json::to_value(&x).unwrap_or_default(); v["extra"] = …`.
  Sites: `api/servers.rs:57,75`, `api/alerts.rs:72,332-333`,
  `api/names.rs:47`, `api/service_accounts.rs:285`.
- **CLI.** `rescmd.rs` has 151 `v["field"]` index expressions, `admincmd.rs`
  94, `cmds.rs` 82, `tunnelcmd.rs` 56. The whole CLI mentions `Deserialize` 5
  times.

### Example

- At `api/service_accounts.rs:285`, a serialisation failure would return `201`
  with a body that holds only the secret, because `unwrap_or_default()` yields
  `null` and the next line adds one key.
- `forge-cli/src/filescmd.rs:647` resolves a node id with
  `["id"].as_str().unwrap_or_default()`. If the response shape changes, the id
  is empty and the CLI requests `/servers//files`.

### Fix

- Response structs with `#[serde(flatten)]` for the "struct plus extras"
  cases. Derive the schema type so OpenAPI describes them.
- Share request and response types between the server and the CLI, in a small
  crate or a module of `forge-core`. Ask before adding a crate.
- In the CLI, treat a missing required field as an error.

### Done when

For the endpoint group you converted: handlers return `Json<T>`, the OpenAPI
document describes the response, the JSON output is unchanged (snapshot or
integration test), and the CLI command for it deserialises into the shared
type.

---

## R4. Store errors swallowed into empty or default results

**Severity:** High. **Size:** small. **Confidence:** about 30 sites listed by
search; `service_accounts.rs` read in full.

### Problem

Server code uses `state.store.<call>().await.unwrap_or_default()` or `.ok()`.
A database failure then renders as "nothing there" and the caller gets a 200.

Sites, under `src/apps/forge-server/src/`:

- `api/service_accounts.rs:60-61`
- `api/servers.rs:48-49,74,77`
- `service_ports.rs:270`
- `api/sites.rs:276,284`
- `api/app_health.rs:274,280`
- `api/tunnels.rs:847`
- `api/package_updates.rs:152,409`
- `health_ingest.rs:90`
- `names.rs:278`

A further 62 serde calls end in `.unwrap_or_default()` or `.ok()`. Many read
stored JSON columns: `api/sites.rs:424,500-505`, `api/fabric.rs:95-96,126`,
`api/routing.rs:230,248`, `names.rs:289`.

`api/idempotency.rs:142` uses `unwrap_or(u64::MAX)`, which fails closed. Leave
it.

### Example

- `api/service_accounts.rs:60-61`: a failed owner lookup shows the owner as
  inactive, and a failed token list shows 0 tokens.
- `service_ports.rs:270`: a failed SSH endpoint lookup silently becomes
  port 22.
- `api/sites.rs:276,284`: an existence check treats a store error as "not
  found".
- A corrupt JSON column reads as an empty list of targets, edges or leases,
  and the reconcile loop then acts on the empty list.

### Fix

- In handlers, propagate with `?`.
- In background loops, log the error and skip the tick. Do not proceed with
  empty data.
- For JSON columns, add typed accessors that return `DomainResult`, for
  example `site.targets()?`.

### Done when

No `state.store` call in the server ends in `.unwrap_or_default()` or `.ok()`
without a comment that says why the default is correct. Check with:
`grep -rnE "store\.[a-z_]+\(.*\)\.await\.(ok\(\)|unwrap_or_default\(\))" src/apps/forge-server/src`.

---

## R5. Oversized functions and long parameter lists

**Severity:** Medium. **Size:** small to medium per function.
**Confidence:** `filescmd.rs:650` read to line 840; `storage.rs:425` read in
full; the rest measured by a rough line counter.

### Problem

Largest files (lines):

| Lines | File |
| --- | --- |
| 2087 | `src/apps/forge-cli/src/admincmd.rs` |
| 1606 | `src/apps/forge-server/src/api/storage.rs` |
| 1525 | `src/protocol/forge-catalog/src/stacks.rs` |
| 1451 | `src/domain/forge-core/src/files.rs` |
| 1336 | `src/services/forge-files/src/lib.rs` |
| 1311 | `src/apps/forge-cli/src/cmds.rs` |
| 1290 | `src/apps/forge-cli/src/filescmd.rs` |
| 1269 | `src/apps/forge-agent/src/files/mod.rs` |
| 1232 | `src/apps/forge-server/src/api/files.rs` |
| 1225 | `src/protocol/forge-catalog/src/lib.rs` |
| 1190 | `src/apps/forge-cli/src/rescmd.rs` |
| 1134 | `src/apps/forge-server/src/api/tunnels.rs` |
| 1126 | `src/apps/forge-cli/src/tunnelcmd.rs` |
| 1088 | `src/apps/forge-server/src/api/mcp.rs` |
| 1031 | `src/apps/forge-agent/src/apps.rs` |

Longest functions (approximate):

| Lines | Location | Verdict |
| --- | --- | --- |
| 555 | `forge-cli/src/filescmd.rs:650 run` | Refactor |
| 504 | `forge-catalog/src/integration.rs:50 integration` | Static data table; leave |
| 272 | `forge-server/src/state.rs:93 build` | Composition root; not read |
| 270 | `forge-server/src/api/mod.rs:105 router` | Route table; leave |
| 256 | `forge-server/src/install/mod.rs:124 render_script` | Not read |
| 241 | `forge-server/src/main.rs:131 main` | Not read |
| 210 | `forge-cli/src/rescmd.rs:677 apps` | Not read |
| 193 | `forge-catalog/src/sites.rs:293 directive_names` | Not read |
| 190 | `forge-server/src/api/storage.rs:425 install` | Refactor (see A1) |
| 181 | `forge-server/src/service_ports.rs:114 catalogue` | Not read |
| 161 | `forge-cli/src/tunnelcmd.rs:358 resolve`, `cmds.rs:519 nodes` | Not read |
| 153 | `forge-catalog/src/apps.rs:334 validate` | Not read |

Sixteen functions carry `#[allow(clippy::too_many_arguments)]`, for example
`api/storage.rs:289 vault_credential` (9 parameters, 4 of them adjacent `&str`
that are easy to transpose), `forge-server/src/stacks.rs:189,217`,
`forge-server/src/update.rs:364`, `forge-files/src/lib.rs:962,1195`,
`api/tunnels.rs:376`, `api/applications.rs:444`.

Signatures with two or more `bool` parameters include
`forge-cli/src/updatecmd.rs:178 run(g, check, yes, allow_downgrade)`,
`forge-server/src/auth.rs:376 token_mfa(mfa_required, elevated, allow_elevated)`
and `Fs::open_write(path, create, truncate)`.

### Example

`filescmd.rs:650 run` is one `match` with an inline arm per subcommand. Each
arm repeats `let id = node_id(&mut api, &node).await?;`. The upload arm
(from about line 780) inlines chunking, hashing, progress and abort cleanup.

### Fix

- `filescmd.rs`: one function per subcommand; resolve the node once.
- `storage.rs install`: split into `validate_layout`, `check_node`, `persist`
  (and see A1 for moving it out of the handler).
- Replace long parameter lists with a parameter struct. Replace paired `bool`
  parameters with an options struct or an enum.

### Done when

The refactored function is under about 80 lines, its
`too_many_arguments` allow is gone, and behaviour is unchanged.

---

## R6. Copy-pasted helpers

**Severity:** Medium. **Size:** small. **Confidence:** definitions listed by
search; the `enc` bodies confirmed identical.

### Problem

| Helper | Copies | Where |
| --- | --- | --- |
| `server_ref(ServerId) -> EntityRef` | 11 | `api/{agent_updates,ingress,logins,names,packages,servers,sites,storage,telemetry,terminal,tunnels}.rs` |
| `ws_ref` | 7 | `api/{access,alerts,certificates,service_accounts,settings,sites,tunnels}.rs` |
| `enc` (URL-encode) | 6 | `forge-cli/src/{admincmd,alertscmd,apicmd,cmds,filescmd,rescmd}.rs` |
| `write_private` | 6 | `forge-ssh/src/ca.rs:95`, `forge-agent/src/fabric.rs:73`, `forge-agent/src/stacks.rs:148`, `forge-cli/src/ctx.rs:117`, `forge-file-gateway/src/lib.rs:75`, `forge-secrets/src/provider.rs:85` (public); four different error types |
| `now_ms` | 6 | agent `files/mod.rs:388`, `health.rs:382`, `self_update.rs:57`, `tunnel.rs:167`; `forge-cli/src/api.rs:101`; `api/onboarding.rs:62` (returns `u64`, the others `i64`). All bypass the `Clock` port |
| `valid_name` | 7 | Different rules under one name: `api/clusters.rs:21`, `api/tenancy.rs:21`, `api/service_accounts.rs:45`, `forge-catalog/src/names.rs:73`, `forge-catalog/src/health.rs:330`, `forge-cli/src/ctx.rs:51`, `forge-agent/src/files/windows.rs:42` |
| `load_or_create(path)` | 6 | Key-file types, six error types |
| `sha256_hex` | 4 | `forge-s3/src/sigv4.rs:21`, `forge-server/src/update.rs:249`, `forge-sdk/src/release.rs:50`, `forge-secrets/src/crypto.rs:28` |
| `internal(e) -> ApiError` | 3 | `api/service_ports.rs:16`, `api/stacks.rs:36`, `api/tunnels.rs:285` |

### Example

Seven functions are called `valid_name` and apply different rules. A reader
who knows one assumes the others behave the same.

`write_private` matters for security: the review found one secret-file writer
that sets the mode after creating the file (S13). One correct implementation
removes that class of mistake.

### Fix

- Add `EntityRef::server(id)` and `EntityRef::workspace(id)` constructors in
  `forge-core`. That deletes 18 functions.
- One `enc` in `forge-cli/src/api.rs`.
- One `write_private` and one `sha256_hex`, both already in `forge-secrets`.
  Check crate dependencies still point inward before reusing them from an
  adapter.
- Rename each `valid_name` by what it validates (`valid_cluster_name`,
  `valid_service_account_name`, …), or share one rule where the rules are
  meant to match.
- Replace `now_ms` with the `Clock` port in the server; in the agent and CLI,
  keep one helper per crate.

### Done when

Each helper in the table has one definition per crate at most, and
`valid_name` no longer names two different rules.

---

## R7. Blocking I/O inside async functions

**Severity:** Medium. **Size:** small. **Confidence:** per-file counts; the
`apps.rs` helpers and the `update.rs` line list read.

### Problem

- `forge-server/src/update.rs:364 apply` is `async` and calls `std::fs::*` and
  `std::process::Command::output()` three times (lines 412, 424, 437) to run
  the new server binary. That blocks a runtime worker thread for the length of
  a migration check.
- `forge-agent/src/apps.rs` has 23 async functions, 41 `std::fs` calls and no
  `spawn_blocking`. The same holds for `stacks.rs` (13), `sites.rs` (10) and
  `fabric.rs` (8). Practical impact is limited: long-running agent commands
  run in a spawned task and some of this runs in the helper process.

### Fix

- Wrap `update::apply` in `spawn_blocking`, or use `tokio::process`.
- In the agent, follow the pattern `files/mod.rs:296` already uses: sync file
  system calls only inside `spawn_blocking`.

### Done when

`update::apply` makes no blocking call on a runtime thread.

---

## R8. Lint policy and the code disagree

**Severity:** Medium. **Size:** small. **Confidence:** config read; offenders
counted.

### Problem

Root `Cargo.toml:107-116` sets `unsafe_code = "deny"` and clippy `all`,
`unwrap_used`, `dbg_macro`, `todo` at warn. There is no `clippy.toml`.

- `CLAUDE.md` bans `expect()` outside tests, but `clippy::expect_used` is not
  enabled. Known non-test use: `src/adapters/forge-ssh/src/ca.rs:135`
  (`.expect("signed by the CA and currently valid")`, not opened).
  `src/services/forge-secrets/src/crypto.rs:16` is allowed and justified.
- Tests avoid `unwrap_used` in two ways: 115 `#[allow(clippy::unwrap_used)]`
  attributes (58 item-level, 54 file-level, 3 combined), and 50 workarounds
  across 23 files such as `.unwrap_or_else(|e| panic!("{e}"))` and
  `.unwrap_or_else(|| unreachable!())` (`forge-server/src/challenge.rs:136-186`,
  `install/mod.rs:527-581`, `api/schema_types.rs:229-268`,
  `api/mcp.rs:1015-1064`).

### Fix

1. Add `#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]`
   once per crate root.
2. Delete the 115 per-module allows and the 50 workarounds (use plain
   `unwrap()` in tests).
3. Enable `expect_used = "warn"` in `[workspace.lints.clippy]`.
4. Fix or justify what the new lint reports. CI runs clippy with
   `-D warnings`, so the workspace must be clean in the same change.

### Done when

`cargo lint` passes with `expect_used` enabled, and
`grep -rn "allow(clippy::unwrap_used" src | wc -l` equals the number of crates
that need it.

---

## R9. Tests with fixed sleeps and duplicated helpers

**Severity:** Medium. **Size:** small. **Confidence:** eight sites read in
context.

### Problem

`CLAUDE.md` says tests poll with deadlines and never use fixed sleeps. Most of
the 33 `sleep` calls under `tests/` are inside poll loops, which is fine.
These are fixed sleeps:

- `forge-server/tests/it/agent_rollout.rs:88,118,203`: 500 ms after stopping
  an agent.
- `forge-server/tests/it/alerts.rs:183`: 5 s.
- `forge-server/tests/it/device_login.rs:63,113`: 5 s each, honouring the
  client poll interval.
- `forge-server/tests/it/sftp.rs:390`: 300 ms, then asserts the session is not
  closed.
- `src/services/forge-jobs/tests/engine.rs:212`: 400 ms waiting for a lease to
  expire.
- `forge-server/tests/it/agent_binary_smoke.rs:101`: 5 s "stays up" check.
  Defensible as a negative assertion; leave it with a comment.

Wait helpers are duplicated: `wait_status` (`agent_link.rs:47`,
`object_storage.rs:651`), `wait_online` (`tunnels.rs:50`), `wait_for`
(`alerts.rs:14`), `wait_closed` (`sftp.rs:295`), `wait_for_status`
(`engine.rs:157`). `enroll_agent` is defined in both `agent_rollout.rs:13` and
`sftp.rs:55`. `tests/common/mod.rs` only has `wait_job`.

Some tests cover several behaviours: `files.rs:153` (259 lines), `scale.rs:44`
(217), `mcp.rs:125` (192), `stacks.rs:84` (179).

### Fix

- One generic `wait_for(what, deadline, f)` and one `enroll_agent` in
  `tests/common`.
- Make the device-login poll interval configurable in tests.
- Use the test clock (`ManualClock`) for the lease-expiry test.
- Split the giant tests when you next touch them.

Use the `test-engineer` subagent.

### Done when

No fixed sleep remains outside a poll loop, except the one commented negative
assertion.

---

## R10. Audit action names and error codes are free-form strings

**Severity:** Medium. **Size:** small. **Confidence:** counted, samples read.

### Problem

- About 119 distinct audit action literals at `caller.audit("…")`,
  `audit_denied` and `require_mfa` call sites, plus 25 `AuditEvent::new`
  sites. Success and denial repeat the literal: `"server.revoke"` at
  `api/servers.rs:195,212`; `"names.suffix.update"` at `api/names.rs:210,222`.
- About 35 distinct error codes in `ApiError::new(status, "code", …)`.
  `"throttled"` is rebuilt 12 times and `"step_up_required"` 5 times.

### Example

A typo in one of two matching audit literals means the success and the denial
of one operation are recorded under different names, and a search of the audit
log misses half.

### Fix

- A `mod audit_actions` of constants, or an enum with `as_str`. The docs page
  that lists audit actions can then be checked against it.
- `ApiError::throttled()` and `ApiError::step_up_required()` constructors
  beside the existing `bad_request` and `not_found`.

### Done when

Audit action names and error codes are each defined once.

---

## R11. A test asserts against a route that does not exist

**Severity:** Medium. **Size:** small. **Confidence:** read.
**Related security finding:** S19.

### Problem

`api/idempotency.rs:245` checks `"/api/v1/api-keys/token"`. The real routes
are `/api-keys` and `/api-keys/{id}` (`api/mod.rs:220-221`). The test passes
only because the path contains the substring `token`. Secret-bearing routes
are identified by substring (`"token"`, `"grant"`, `"reveal"`), not by a
marker on the route.

### Fix

Tag secret-bearing routes in the route table (`build_routes.rs` already
generates route metadata) and make the test iterate real routes. Fix S19 in
the same change.

### Done when

The test fails if a route that returns a `secret` field is replayable or
compressible.

---

## R12. Per-row store and authorize calls in list endpoints

**Severity:** Medium. **Size:** small per endpoint. **Confidence:** rough
search; `service_accounts.rs` read. Not checked: whether the authorization
engine caches per request. If it does, the `can` loops are cheap.

### Problem

- `api/service_accounts.rs:79-88`: three queries per account.
- Per-row `caller.can(...)` although `can_many` exists and 12 handlers use it:
  `api/applications.rs:73`, `api/clusters.rs:47`, `api/jobs.rs:40`,
  `api/stacks.rs:112` (plus `list_nodes` per stack at 115),
  `api/tenancy.rs:88,249`, `api/telemetry.rs:74,105` (plus `get_ssh_endpoint`
  per server at 108), `api/tags.rs:100`, `api/access.rs:336`,
  `api/ingress.rs:311`.
- Per-row lookups: `api/storage.rs:240` and `:514-518`, `api/fabric.rs:76`,
  `api/terminal.rs:204`, `service_ports.rs:132,270`, `names.rs:331`,
  `health_ingest.rs:92`.

### Fix

Move to `can_many`. Add batch store methods (`get_users(&[id])`,
`ssh_endpoints_for(&[id])`) as new port methods.

### Done when

The listed endpoints make a constant number of store calls regardless of row
count. Measure before and after if you claim a speed-up.

---

## R13. Low-impact Rust items

Fix when you are in the file.

- **Dead-code allows.** `api/schema_types.rs:9` has a file-wide
  `#![allow(dead_code)]`; `api/audit.rs:30` and
  `forge-agent/src/service/mod.rs:72` have item-level ones (not opened).
- **Misplaced comment.** `forge-agent/src/agent.rs:187-189` describes
  cancelling a background operation but sits above the tunnel-open block. The
  cancel code is at line 211.
- **"Legacy" paths.** `forge-catalog/src/storage.rs` (18 mentions),
  `forge-agent/src/apps.rs` (8, including `rename_legacy` at line 202),
  `forge-server/src/jobs/storage.rs` (7), `forge-cli/src/main.rs` (7). Counted
  only. Check whether these migration paths can be retired.
- **Unbounded channels.** `forge-agent/src/agent.rs:91` for background
  results; three more for log lines in `helper.rs:527,541` and
  `forge-agent-helper.rs:265`.
- **Ignored mutex poisoning.** `agent.rs:211,251,258` use
  `self.running.lock().ok()`. After a panic, cancel silently reports
  `cancelled: false`.
- **Dropped task handles.** 31 `tokio::spawn` calls in statement form outside
  the tracked background loops (for example in `api/system_update.rs`,
  `api/fabric.rs`, `api/clusters.rs`, `forge-file-gateway/src/lib.rs`). A
  panic in those tasks is lost. Not opened individually.
- **Store layer.** `row.rs:38 u32_col` clamps an out-of-range value to
  `u32::MAX` silently. `NodeRole::parse` and `AgentStatus::parse`
  (`servers.rs:17,22`) appear to map unknown values to a default (not opened).
  Each list method duplicates its query for `Some(workspace)` and `None`
  (`servers.rs:95-103`, `alerts.rs:170-175`, `object_stores.rs:114-119`).
- **Naming.** `api/service_accounts.rs:297 revoke_token` authorizes
  `Action::UserManage` while every other mutation in the file uses
  `AccessManage`; confirm which is intended and document it.
  `service_accounts.rs:280` writes both `"token"` and `"token_id"` with the
  same value. `list_servers(org: Option<WorkspaceId>)` still names the
  parameter `org` (`servers.rs:94,108`, `auth.rs:110`).
  `forge-server/src/tunnels.rs:436` has a method named `expect`, which reads
  like `Option::expect` at `api/tunnels.rs:728`. `ServiceAccount.created_by`
  is a `String` where a typed id exists.

---

# Part C: the web console (`src/ui`)

UI metrics at review time:

| Metric | Value |
| --- | --- |
| Files over 400 lines / over 600 lines | 31 / 11 |
| Test files, test runner, ESLint config | 0 / none / none |
| `any` (real uses) | 1 (`AppEditor.tsx:437`) |
| `as unknown as` / `as never` | 3 / 10 (9 in `Alerts.tsx`) |
| `eslint-disable` comments | 23 (21 are `react-hooks/exhaustive-deps`) |
| `useState` / `useReducer` | 657 / 0 |
| `useEffect` | 107 |
| `useQuery` + `useMutation` sites | 138; 61 outside `src/api/` |
| `queryKey:` literals / outside `src/api/` / distinct first segments | 167 / 79 / 68 |
| `key={i}` | 37 |
| Native `confirm()` | 3 |
| Files touching `localStorage` directly | 12 |

Use the `ui-engineer` subagent for UI work.

## U1. No ESLint, yet 21 suppressions of a rule nothing enforces

**Severity:** High. **Size:** small to set up, medium to clean.
**Confidence:** read.

### Problem

`src/ui/package.json` has no ESLint dependency and no `lint` script, and there
is no config file. CI (`.github/workflows/ci.yml:254-263`) runs only
`npm ci && npm run build` and `npm audit`. The 107 `useEffect` calls have no
`exhaustive-deps` or `rules-of-hooks` check. The 21
`eslint-disable-next-line react-hooks/exhaustive-deps` comments suppress a
check that does not run.

### Fix

1. Ask the owner before adding the dev dependencies (`eslint`,
   `typescript-eslint`, `eslint-plugin-react-hooks`; all MIT). Run them past
   the `dependency-auditor` subagent.
2. Add `npm run lint` and wire it into CI.
3. Add an unused-exports check (`knip` or an ESLint rule). Three dead exports
   exist today: `useAudit` (`src/api/hooks.ts:162`), `CLUSTER_ITEMS`
   (`src/app/nav.ts:60`), `ActivityIcon` (`src/app/nav.ts:187`). Delete them.
4. Revisit each suppression. Most disappear with U4.
5. Update `docs/src/content/docs/development/ui.md` and `contributing.md`.

### Done when

`npm run lint` passes in CI and every remaining suppression has a comment
that says why.

---

## U2. No tests

**Severity:** High. **Size:** medium. **Confidence:** read.

### Problem

No `*.test.*` or `*.spec.*` file, no test runner.

### Fix

Ask before adding `vitest` (MIT). Start with pure logic, which needs no
browser:

- `fromComposeYaml`, `toComposeYaml`, `parseDotenv` in
  `src/components/AppEditor.tsx:67,407,436`
- `toWire`, `fromWire`, `relName` in `src/pages/Dns.tsx:51-77`
- `src/components/health/model.ts` (411 lines of draft, policy and YAML
  conversion)
- `src/components/stacks/diff.ts`
- subnet maths in `FabricForm.tsx:65-90`
- the normalizers in `src/api/hooks.ts` (`normalizeLines`, namespaces,
  onboarding nodes)
- `bytes`, `duration`, `ago`, `bytesRate`
- step-up retry in `src/api/client.ts:49-51`
- chunked upload and resume in `src/api/files.ts:262-300`

Then a few browser smoke tests for the riskiest flows: login with MFA step-up,
file edit with a conflict, stack deploy, add node.

### Done when

`npm test` runs in CI and covers the listed pure functions.

---

## U3. The data layer is only half centralised

**Severity:** High. **Size:** medium. **Confidence:** read.

### Problem

Every request goes through `src/api/client.ts`, which is good. The layer above
it is inconsistent:

- **Queries defined in components.** 61 of 138 query and mutation sites live
  outside `src/api/`, with inline URLs and inline response types.
  `Endpoints.tsx:155` and `ProfileEditor.tsx:119` each carry a multi-line
  anonymous response type inside the `post<…>` call.
- **The same query defined in several places.** `["certificates"]` has 6
  occurrences across `Certificates.tsx:89`, `Endpoints.tsx:132,328` and
  `StackDialogs.tsx:254`; only the last normalises and catches. `["sites"]`
  has 8 occurrences. `["users"]` is in `hooks.ts:182` and
  `ConnectedApps.tsx:58`. `["fabric", clusterId]` is in `IpTag.tsx:13` and
  `ClusterNetwork.tsx:19`.
- **No key factory.** 68 distinct string first-segments. `GrantList.tsx:307-308`
  has to know both `["file-grants"]` and `["all-file-grants"]`.
- **Three invalidation styles.** `useAction(fn, keys)` (`hooks.ts:292`, 54
  uses) is the good one but is not exported. There are also 40 hand-written
  `useMutation` + `onSuccess` blocks and a string loop at
  `ClusterWizard.tsx:125`. The WebSocket handler (`hooks.ts:459-481`)
  hard-codes a further key list.
- **Server state in `useState` + `useEffect`.** See U7.

### Example

One cache key, `["certificates"]`, holds data with different guarantees
depending on which component fetched first. A typo in a key string silently
breaks invalidation, and the page shows stale data after a mutation.

### Fix

1. Add `src/api/keys.ts` as a key factory.
2. Move the certificates, sites, fabric, settings, API keys, tags and vault
   queries into `src/api/<domain>.ts`. Split `hooks.ts` (1,184 lines) by
   domain, as `files.ts`, `stacks.ts` and `storage.ts` already are.
3. Export `useAction` and use it for every mutation.
4. Make the WebSocket invalidation table reference the key factory.

### Done when

No `useQuery` or `useMutation` call exists outside `src/api/`, and no
`queryKey` string literal exists outside `src/api/keys.ts`.

---

## U4. Oversized components with one `useState` per field

**Severity:** High. **Size:** medium per component. **Confidence:** read.

### Problem

- `FileExplorer`, `src/pages/Files.tsx:87-636` (about 550 lines, 18
  `useState`)
- `InstallStoreDialog`, `src/components/storage/StoreDialogs.tsx:110-533`
  (about 420 lines)
- `ApplicationDetail`, `src/pages/ApplicationDetail.tsx:60-424` (about 365
  lines)
- `Dns`, `src/pages/Dns.tsx:80-435` (about 355 lines, 11 `useState`)
- `Assisted`, `src/pages/AddNode.tsx:88` (14 `useState`)

Most `useState` per file: `StackDialogs.tsx` 24, `ManageDialogs.tsx` 23,
`Dns.tsx` 21, `AddNode.tsx` 21, `NewContainer.tsx` 20, `Alerts.tsx` 19,
`StoreDialogs.tsx` 19, `Files.tsx` 18.

Dialogs are always mounted, and a `useEffect(…, [open])` resets every field by
hand: 23 such effects, 16 with a suppression. Examples:
`ClusterWizard.tsx:35-48` (11 setters), `StoreDialogs.tsx:716-731` (11
setters), `StoreDialogs.tsx:119-126,562-571`, `MoveClusterDialog.tsx:31-37`,
`Dns.tsx:456,572,656`.

### Example

A developer adds a field to a dialog and forgets the matching setter in the
reset effect. The next time the dialog opens, it shows the previous value.

### Fix

- Render dialogs conditionally (`{open && <XDialog/>}`) or key them
  (`key={openCount}`), so the initial state is the reset. That deletes the
  effects and most suppressions.
- Hold form state as one object. `InstallStoreDialog` already does this with
  `InstallForm` and `set(partial)` at `StoreDialogs.tsx:128`. Apply it to
  `ConnectStoreDialog`, `Assisted`, `RecordDialog` and `NewContainer`. A small
  `useForm(initial)` helper is enough; do not add a form library.
- Split `FileExplorer` into a selection-and-actions hook, a toolbar and a
  listing.
- Move the DNS wire helpers and the three DNS dialogs into `components/dns/`.

### Done when

The component is under about 250 lines, has no reset-on-open effect, and
behaves the same in both themes and at narrow widths (check it in the running
app).

---

## U5. Hand-written types with no link to the server's OpenAPI document

**Severity:** High. **Size:** medium. **Confidence:** read. Not verified:
whether the types have already drifted from the server.

### Problem

The server serves `GET /api/openapi.json`
(`src/apps/forge-server/src/api/mod.rs:300`). The UI never uses it.
`src/api/types.ts` (940 lines) is maintained by hand, and about 100 more
`interface` and `type` declarations sit in pages and components.

Duplicates already exist on the client:

- `Site` and `EdgeCert`: `Certificates.tsx:12,21` and `Endpoints.tsx:28,76`
- `Capabilities`: `types.ts:283` and `ProcessPanel.tsx:46`
- `Revision`: `api/files.ts:87` and `api/stacks.ts:124`
- `DiffLine`: `health/model.ts:382` and `stacks/diff.ts:3`

Some hooks defend themselves with `get<unknown>` plus a normalizer. Others
trust `get<T>` unchecked: `useClusters`, `useServers`, `useBindings`
(`hooks.ts:105-109,183`).

### Fix

- Preferred: generate types from the spec with `openapi-typescript`
  (dev-only, MIT; ask first) into `src/api/schema.d.ts`, and make `types.ts`
  re-export or alias them. This depends on R3: endpoints that return
  `Json<Value>` have no schema to generate from.
- Otherwise: a CI check that compares the spec's schema names and fields with
  `types.ts`.
- Move the ingress types into `src/api/ingress.ts` and delete the duplicates.

### Done when

No type is declared twice, and CI fails when a server response type changes
without the UI type changing.

---

## U6. Loading and error handling differs per page

**Severity:** Medium. **Size:** small. **Confidence:** examples read; page
counts are search results (some pages delegate to child components).

### Problem

22 of 42 pages never use `ErrorNote`, and 13 never use `Skeleton`. These hide
a failed request as an empty state:

- `Dns.tsx:82-84` (`useServers().data ?? []`)
- `Alerts.tsx:203-205` (a failed rules query renders "No rules yet")
- `Endpoints.tsx:132` (`.data ?? []` on certificates)

`retry: false` appears 38 times beside the default of 3 retries, with no
stated rule.

### Fix

A small `<QueryState q={…} empty={…}>` wrapper, or a `useList(q)` helper that
returns `{rows, loading, error}`, used by every table page. Decide the `retry`
default once in `src/main.tsx:19`.

### Done when

A failed list request shows an error on every table page, not an empty state.

---

## U7. Hook bugs and derived-state effects

**Severity:** Medium. **Size:** small each. **Confidence:** read.

### Problem and fix, per site

- **Socket leak.** `src/api/hooks.ts:484-493`: the reconnect `setTimeout` is
  not cleared in cleanup, and `connect()` does not check `closed`. Signing out
  while disconnected opens a socket that never closes. Fix: store the timer
  id, clear it in cleanup, return early in `connect` when `closed`.
- **Unguarded parse.** `hooks.ts:457` calls `JSON.parse(m.data)` with no
  try/catch.
- **Fetch races.** `Dns.tsx:86-131` keeps zones and records in local state
  with no cancellation, so a fast zone switch can show the previous zone's
  records. `FileViewer.tsx:91-128` has the same race on `path`. Fix: convert
  to `useQuery` with keys `["dns", node, "records", zone]` and
  `["file", server, root, path]`.
- **Overlapping polls.** `useNodeMetrics` (`hooks.ts:778-813`) uses a
  hand-rolled `setInterval`; ticks can overlap. `LiveLogs.tsx:45-92` is a
  manual polling loop that only stops on unmount (`:94-96`).
  `FabricForm.tsx:101-117` fetches in a mount effect. Fix: use query options
  (`refetchInterval`).
- **Lost edits.** `ProfileEditor.tsx:79`:
  `useEffect(() => setP(saved), [saved])` overwrites unsaved edits on any
  refetch. Fix: key the editor on the saved identity, or only reset when not
  dirty.
- **Derived state through effects.** `Files.tsx:119`
  (`useEffect(() => setDraft(path), [path])`), `Files.tsx:120-123`,
  `Members.tsx:119` (page reset on filter change), `LiveLogs.tsx:41-43`. Fix:
  key on the identity or compute during render.
- **Stale closure.** `HealthTab.tsx:106-137` reads `base`, `dirty`, `mode`
  and `editing` under two suppressed dependency arrays. A reducer fits this
  state.
- **No-op effect.** `AppEditor.tsx:480` repeats the `useState` initializer in
  an effect with `[]`. Delete it.

### Done when

Each listed site is fixed and has a test where the logic is testable (the
socket reconnect and the DNS switch are).

---

## U8. Interaction and accessibility

**Severity:** Medium. **Size:** small. **Confidence:** listed sites read;
counts are search results.

- **Native `confirm()`:** `Alerts.tsx:229`, `Files.tsx:274` (a permanent bulk
  delete), `FileViewer.tsx:323`. Use the shared `Confirm` dialog.
  `Dns.tsx:639` defines a private `TypedConfirm`; move it to `ui.tsx`.
- **Mouse-only rows:** `<tr onClick>` with no `tabIndex` or key handler at
  `Alerts.tsx:82`, `Approvals.tsx:132`, `Jobs.tsx:69`, `NodeTable.tsx:239`.
  Make the row's primary cell a link or a button.
- **Button type:** `Button` and `IconButton` in `ui.tsx` do not default
  `type="button"` (`CopyIcon` and `Switch` do). Default it. 142 raw `<button>`
  elements exist against 346 `<Button>` uses.
- **Index keys on editable lists:** `AppEditor.tsx:135,176,325`,
  `Dns.tsx:508,600`, `NewContainer.tsx:44`, `TargetEditor.tsx:117`,
  `CheckCard.tsx:198,246`. Use a stable id, as `StoreDialogs.tsx:73`
  (`memberSeq`) and `health/model.ts:28` (`newKey`) already do. The other
  `key={i}` uses are static lists and fine.
- **Error boundaries:** `RouteError` (`router.tsx:193`) is the only one. A
  throw in one tab of `NodeDetail` or `ApplicationDetail`, or in
  `TerminalDock`, `JobsDrawer` or `CommandPalette` (rendered outside
  `<Outlet>` in `Layout.tsx`), takes down the route or the shell. Add a small
  reusable `Boundary` around each tab panel and around the shell-level drawers
  and dock.

---

## U9. Duplicated UI helpers and preferences

**Severity:** Medium. **Size:** small. **Confidence:** read.

- **Bytes, three implementations with different unit labels:** `ui.tsx:43`
  (`KiB`), `ProcessPanel.tsx:93` (a local `bytes` that shadows the shared
  name), `TimeSeries.tsx:185` (`fmtBytes`). Rate formatters:
  `Traffic.tsx:48` (`bytesRate`) and `TimeSeries.tsx:195` (`fmtRate`).
- **Uptime, twice:** `NodeOverview.tsx:25` and `ProcessPanel.tsx:111`.
- **Error text:** `errorText` exists in `toast.tsx:50`, but about 15 sites
  re-inline `e instanceof Error ? e.message : String(e)`, including
  `hooks.ts:996`, `webauthn.ts:49`, `ui.tsx:343`, `approvals.ts:112,161`,
  `connectors.ts:141,168,233`.
- **Dates:** raw `toLocaleString()` at `Alerts.tsx:139,144,174` instead of
  `time()` and `ago()`.
- **Progress bars:** the same bar markup is re-implemented in about 10 files
  (`NodeTable.tsx:27`, `NodeOverview.tsx:43,243`, `ProcessPanel.tsx:125`,
  `Certificates.tsx:63`, `Health.tsx:69`, `UploadDialog.tsx:330`,
  `ObjectExplorer.tsx:118`, `Clusters.tsx:167`, `Storage.tsx:45`,
  `AgentUpdates.tsx:440`).
- **Preferences:** `usePrefs` (`src/app/prefs.tsx`) only knows theme,
  namespace and sidebar. Eight files re-implement a try/catch `localStorage`
  read and write: `AppStore.tsx:52-60`, `Ingress.tsx:21-28`,
  `NewStack.tsx:25,65`, `ProcessPanel.tsx:239-259`, `FleetNav.tsx:19-28`,
  `Notifications.tsx:38-47`, `CommandPalette.tsx:45,216`,
  `LoginPicker.tsx:27-37`.

**Fix:** move the formatters to `src/lib/format.ts` (one byte formatter, one
unit convention), move `errorText` to `src/lib/`, add a `Meter` component to
`ui.tsx`, and export a generic `usePref<T>(key, fallback)` from `prefs.tsx`.

**Done when:** each helper has one definition and the same value renders with
the same unit everywhere.

---

## U10. Low-impact UI items

- **Type escapes in `Alerts.tsx`:** nine `as never` casts
  (`:84-127,215,288,388`) work around `Badge`'s tone type. Type `SEV_TONE` and
  `STATE_TONE` as `Record<Severity, Tone>` and export `Tone` from `ui.tsx`.
  Also `Nodes.tsx:29` (`search as never`) and the fake `ChangeEvent` at
  `SearchSelect.tsx:87`.
- **The one `any`:** `fromComposeYaml` (`AppEditor.tsx:437`) reads user YAML
  unchecked. Type it as `Record<string, unknown>` with small guards, and move
  the YAML codec to `src/lib/compose.ts`.
- **Bundle:** `Applications.tsx:7` and `ClusterServices.tsx:10` import only
  the `RUNTIMES` constant from `AppEditor.tsx`, which statically imports
  `js-yaml`. Move `RUNTIMES` to its own module.
- **Dependencies:** `@radix-ui/react-tooltip` is in `package.json` and
  imported nowhere. `@types/js-yaml` is in `dependencies` and belongs in
  `devDependencies`.
- **Layering:** `components/PackageList.tsx:8` imports `categoryIcon` from
  `pages/AppStore`. `components/` has 37 root files beside feature folders;
  `SettingsSections.tsx`, `AgentUpdates.tsx` and `AppEditor.tsx` would sit
  better in feature folders.
- **`ui.tsx`:** 29 components plus formatters in 673 lines. Split into `ui/`
  with a re-export when next touched.
- **Styling:** hard-coded colours at `Sidebar.tsx:82,111` and
  `TerminalPane.tsx:444` (`terminal/themes.ts` is a legitimate exception).
  `text-[11px]` 289 times and `text-[13px]` 194 times, plus half-pixel
  one-offs; define `--text-2xs` and `--text-sm-dense` in `styles.css`.
  `Layout.tsx:63` hard-codes `ml-[120px]` and `ml-[280px]` while
  `Sidebar.tsx:51` uses `SIDEBAR_WIDTH`.
- **`router.tsx`:** the same `validateSearch` string guard is repeated about
  12 times; add an `optStr(s.x)` helper beside `str` at `:47`.

---

# What to keep

Do not refactor these away. New code should follow them.

**Rust**

- Compiler-enforced layering: no service, domain or protocol crate depends on
  an adapter.
- `JobHandler` + `JobRegistry` (`src/services/forge-jobs/src/handler.rs`): the
  engine has no `match` on kind.
- `Caller` as the single authorize-and-audit helper, with denials audited
  inside `authorize` (`src/apps/forge-server/src/auth.rs:86-95`).
- `Idempotency-Key` as middleware (`api/mod.rs:291`), not per handler.
- One `ApiError` and one `IntoResponse`, with internal errors masked.
- Typed ids (UUID v7, one macro in `ids.rs`) and the injected `Clock` with
  `ManualClock`.
- `Action::ALL` checked against the Cedar schema
  (`src/services/forge-authz/src/engine.rs:119`).
- `forge-catalog` sharing one validation between the control plane and the
  helper.
- `forge-files` as the model service: ports in, in-memory fakes in tests.
- `forge-store`: one row mapper per entity, shared decoding helpers in
  `row.rs`, transactions inside the adapter.
- Server background loops tracked and joined on shutdown
  (`src/apps/forge-server/src/lib.rs:93-135`).
- `spawn_blocking` for CPU-heavy work.
- No TODO, FIXME or commented-out code; doc comments that explain why.

**UI**

- Strict TypeScript with `noUncheckedIndexedAccess`, `noUnusedLocals` and
  `noUnusedParameters`.
- One small API client with CSRF and step-up retry in one place, and no tokens
  in browser storage.
- Defensive hooks: `get<unknown>` + normalizer + `.catch` default.
- Conditional `refetchInterval` with WebSocket invalidation as the primary
  path.
- `useAction` for mutation plus invalidation.
- Route-level code splitting, lazy xterm and CodeMirror, typed search params.
- Token-only theming and the shared `ui.tsx` components.
- Radix for every dialog, menu and popover.

---

# Status

Update this table in the pull request that finishes an item.

| ID | Item | Severity | Status |
| --- | --- | --- | --- |
| A1 | Business logic in handlers; concrete store exposed | High | Open |
| A2 | Typed `NodeCommand` | High | Open |
| A3 | Process-global state | Medium | Open |
| A4 | Stringly-typed model and errors | Medium | Open |
| A5 | Port gaps | Medium | Open |
| A6 | Two grant models | Medium | Open |
| A7 | Settings registry | Medium | Open |
| A8 | Pagination and handler consistency | Medium | Open |
| A9 | PostgreSQL tests in CI | Medium | Open |
| A10 | Smaller structural items | Low | Open |
| R1 | String errors in the agent | High | Open |
| R2 | String actions, statuses, OS | High | Open |
| R3 | Untyped JSON responses and CLI | High | Open |
| R4 | Swallowed store errors | High | Open |
| R5 | Oversized functions | Medium | Open |
| R6 | Duplicated helpers | Medium | Open |
| R7 | Blocking I/O in async | Medium | Open |
| R8 | Lint policy | Medium | Open |
| R9 | Test sleeps and helpers | Medium | Open |
| R10 | Audit and error-code constants | Medium | Open |
| R11 | Idempotency test on a non-existent route | Medium | Open |
| R12 | Per-row queries | Medium | Open |
| R13 | Low-impact Rust items | Low | Open |
| U1 | ESLint | High | Open |
| U2 | UI tests | High | Open |
| U3 | Data layer | High | Open |
| U4 | Oversized components | High | Open |
| U5 | Generated types | High | Open |
| U6 | Loading and error states | Medium | Open |
| U7 | Hook bugs | Medium | Open |
| U8 | Interaction and accessibility | Medium | Open |
| U9 | Duplicated UI helpers | Medium | Open |
| U10 | Low-impact UI items | Low | Open |
