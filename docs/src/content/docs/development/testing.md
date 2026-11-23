---
title: Testing
description: What the test suites prove and how the acceptance criteria map to tests.
sidebar:
  order: 2
---

`cargo test --workspace` runs about 130 tests. Integration tests use real
components: SQLite, Cedar, TLS, the agent library, the plugin runner binary and
the server binary. Nothing security-relevant is mocked.

## Acceptance criteria → tests

| Criterion | Test |
| --- | --- |
| Unauthorized actions denied at the **API** | `forge-server/tests/it/api_authz.rs` — `roles_are_enforced_at_the_api`, `cookie_sessions_require_csrf_for_mutations`, `org_admin_cannot_escalate`, `revoking_a_role_takes_effect_immediately` |
| … at the **job runner** | `forge-jobs/tests/engine.rs` — `runner_denies_when_role_revoked_after_submission`, `unauthorized_submission_is_denied`; `agent_link.rs` — `unauthorized_users_cannot_command_servers` |
| … at the **agent** | `forge-server/tests/it/agent_link.rs` — `agent_local_policy_denies_unlisted_commands`; `forge-proto` signing tests (bad signature, wrong target, expiry, replay, restart boundary) |
| Kill mid-job → resume | `forge-server/tests/it/resume.rs` — real binary, hard kill, restart; `forge-jobs` — `crashed_worker_job_resumes_from_last_incomplete_step` |
| Tampered audit entry detected | `forge-audit/tests/chain.rs` (modify, delete, re-hash forgery); `api_authz.rs` — `tampered_audit_entry_is_detected_via_api` |
| Plugin over budget stopped and quarantined | `forge-plugin-runner/tests/lifecycle.rs` — fuel, wall clock, memory |
| Plugin permissions = user ∩ plugin ∩ policy | `lifecycle.rs` — `effective_permission_is_user_intersect_plugin`, `plugin_sees_only_what_the_user_can_see` |
| Version/capabilities | `api_authz.rs` — `version_and_capabilities_are_public` |

## Per crate

| Crate | Focus |
| --- | --- |
| `forge-authz` | role × action matrix, scope isolation, MFA guardrail, strict validation |
| `forge-store` | migrations, leases and fencing, idempotency, single-use tokens, resolver |
| `forge-identity` | RFC 6238 and RFC 7636 vectors, CSRF, PKCE, refresh rotation, OIDC linking |
| `forge-pki` | CSR subject override, CA persistence, token format |
| `forge-secrets` | envelope round trip, AAD binding, tamper detection |

## Writing tests

- Use `tempfile` directories and real SQLite; tests run in parallel.
- Poll for conditions with a deadline; never rely on a fixed sleep.
- Assert on denial *and* on the audit entry it produced where relevant.

## Test layout

Every integration test binary links its whole crate statically. For
`forge-server` that is about 70 MB of code and 140 MB of debug info per binary,
so a crate's integration tests share **one** binary instead of one per file:

| Path | Contents |
| --- | --- |
| `tests/it/main.rs` | Declares each test file as a module, and the shared harness once |
| `tests/it/<area>.rs` | The tests for one area, such as `api_authz.rs` or `files.rs` |
| `tests/common/mod.rs` | `forge-server` only: the in-process server harness, used as `crate::common` |

`forge-server` and `forge-store` use this layout. To add tests, create
`tests/it/<area>.rs` and add `mod <area>;` to `tests/it/main.rs`. Don't add a
new top-level `tests/<area>.rs`: it becomes another binary that links the whole
crate again.

All modules run in one process, in parallel. Keep each test independent:

- Bind `127.0.0.1:0` and read the port back; never use a fixed port.
- Use your own `tempfile` directory; never change the current directory.
- Don't call `std::env::set_var` at runtime. Pass settings through the config
  instead.

Run one area's tests with a module filter:

```sh
cargo test -p forge-server --test it api_authz::
```
