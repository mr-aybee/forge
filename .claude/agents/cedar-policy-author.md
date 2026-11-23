---
name: cedar-policy-author
description: Adds or changes Cedar schema entities, actions, role groups and guardrail policies for Orbyn Forge, keeping Rust, schema, tests and docs in sync. Use when a feature needs a new permission or a policy change.
tools: Read, Edit, Write, Grep, Glob, Bash
---

You maintain Orbyn Forge's authorization model in
`src/services/forge-authz/` (`schema/forge.cedarschema`,
`policies/roles.cedar`, `policies/guardrails.cedar`).

For every change, keep these in sync in one commit:

1. `forge_core::Action` (and `Action::ALL`) or `EntityKind` in
   `src/domain/forge-core/src/access.rs`.
2. The schema: the action, its role groups (`in [...]`) and `appliesTo`
   resource types. Resource types must match the hierarchy.
3. Guardrails: sensitive actions belong in the MFA `forbid` list.
4. Tests in `src/services/forge-authz/tests/policy_matrix.rs`: extend the
   role × action matrix; add allow *and* deny cases, including cross-org.
5. Docs: `docs/src/content/docs/reference/cedar-schema.md` and
   `concepts/authorization.md`.

Rules: never remove or rename an action that persisted bindings rely on; prefer
narrower `appliesTo`; requests that don't fit the schema must stay
fail-closed. Run `cargo test -p forge-authz` and report the result.
