---
title: Plugins
description: Signed WebAssembly components, lifecycle, permissions, budgets and quarantine.
sidebar:
  order: 6
---

Every integration beyond the core is a signed, versioned plugin. Phase 1
implements the Wasm plugin model; declarative modules and external
integrations follow.

## Lifecycle

```diagram
alt: Plugin lifecycle states and transitions
states:
  path:
    - { state: upload, edge: "" }
    - { state: verify, edge: "" }
    - { state: approve permissions, edge: "" }
    - { state: installed, edge: enable, tone: accent }
    - { state: enabled, dir: both, tone: ok }
    - { state: disabled }
  transitions:
    - { from: installed, to: upgrade, when: new version }
    - { from: enabled, to: quarantined, when: budget or faults, tone: warn }
    - { from: disabled, to: uninstall, when: no dependents left, tone: danger }
```

| Operation | Rules |
| --- | --- |
| Install | Signature from a trusted publisher; the id must be `<publisher>.<name>` of that publisher; API range must match; approved ⊆ requested |
| Enable / disable | Disabling stops new invocations; quarantine is cleared only by re-enabling |
| Upgrade | Version must increase; same publisher as the installed plugin; pinned plugins refuse |
| Pin / unpin | Freeze a version |
| Uninstall | Removes the integration only — never the infrastructure it managed |

All of these require `plugin.manage` (platform admin with MFA) and are audited,
including refused attempts. Every invocation is audited as `plugin.invoke`.

## Effective permission

> **user** ∩ **plugin's approved permissions** ∩ **resource policy**

When a plugin calls `server-online("web-01")`:

1. Is `host:server-online` approved for this plugin? If not → refused.
2. Does the invoking user have `view` on that server (Cedar, including
   guardrails)? If not → the server is reported as not online, which is
   indistinguishable from not existing.

## Sandbox and budgets

| Limit | Mechanism |
| --- | --- |
| No files, sockets or clocks | The linker defines only `forge:plugin/host`; WASI imports fail to instantiate |
| Instructions | Wasmtime fuel |
| Wall time | Epoch deadline + host-side kill |
| Memory | `memory_mb` is a total across all of the plugin's memories; growth beyond it traps |
| Output | Byte budget on host-call arguments and logs |
| Concurrency | Per-plugin semaphore |
| Process | Separate `forge-plugin-runner` per invocation; it checks the artifact's SHA-256 against the installed digest before loading it |

Exceeding any budget stops the run and **quarantines** the plugin. Other
faults quarantine it after three occurrences. A refused host call is not a
fault — that is the sandbox working.

See [Writing plugins](/docs/guides/writing-plugins/) and the
[manifest reference](/docs/reference/plugin-manifest/).
