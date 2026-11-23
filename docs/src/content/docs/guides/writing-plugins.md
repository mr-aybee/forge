---
title: Writing plugins
description: Build, sign, install and invoke a WebAssembly component plugin.
sidebar:
  order: 6
---

This guide shows you how to build a plugin, sign it, and install, enable and
invoke it. A plugin is a WebAssembly component that targets the
`forge:plugin/plugin` world, plus a `forge-plugin.toml` manifest and a
publisher signature. For how plugins are sandboxed, see
[Plugins](/docs/concepts/plugins/).

## Before you begin

- Installing, enabling and disabling plugins is `plugin.manage`: only
  **Platform admin**, with an MFA-verified session.
- Invoking a plugin needs `plugin.invoke` on the scope (**Developer**,
  **Operator**, **Admin** or **Platform admin**).
- To build a Rust plugin you need the `wasm32-unknown-unknown` target and
  [`wasm-tools`](https://github.com/bytecodealliance/wasm-tools):

  ```sh
  rustup target add wasm32-unknown-unknown
  cargo install wasm-tools
  ```

- The control plane needs `forge-plugin-runner` (shipped in every release
  package).

## 1. Read the interface

```wit
package forge:plugin@0.1.0;

interface host {
  log: func(message: string);                    // permission host:log
  server-online: func(hostname: string) -> bool; // permission host:server-online
}

world plugin {
  import host;
  export run: func(input: u32) -> u32;
}
```

The source is `src/protocol/forge-sdk/wit/forge-plugin.wit`.

## 2. Build a Rust plugin

Create a library crate, copy `forge-plugin.wit` into it, and add
`wit-bindgen`:

```sh
cargo new --lib probe && cd probe
cargo add wit-bindgen
```

In `Cargo.toml`, build a C-style dynamic library:

```toml
[lib]
crate-type = ["cdylib"]
```

```rust
wit_bindgen::generate!({ world: "plugin", path: "forge-plugin.wit" });

struct Probe;

impl Guest for Probe {
    fn run(_input: u32) -> u32 {
        forge::plugin::host::log("checking web-01");
        forge::plugin::host::server_online("web-01") as u32
    }
}

export!(Probe);
```

```sh
cargo build --release --target wasm32-unknown-unknown
wasm-tools component new target/wasm32-unknown-unknown/release/probe.wasm -o probe.component.wasm
```

Do not target WASI: the runner provides no WASI imports, and a component that
imports them fails to instantiate.

The repository's sample, `src/plugins/uptime-probe`, is written in the
component text format so it builds without a wasm toolchain.

## 3. Write the manifest

```toml
[plugin]
id = "acme.uptime-probe"
name = "Uptime probe"
version = "0.1.0"
publisher = "acme"
forge_api = ">=0.1.0, <0.2.0"

[permissions]
requested = ["host:log", "host:server-online"]

[budgets]
fuel = 5000000
memory_mb = 4
timeout_ms = 1000
```

See the [manifest reference](/docs/reference/plugin-manifest/).

## 4. Sign the plugin

The signature is Ed25519 over `"forge-plugin-v1\n" || sha256(manifest) || sha256(artifact)`.
Use `forge_sdk::signing::PublisherKey`:

```rust
let (key, pkcs8) = PublisherKey::generate()?;   // keep pkcs8 secret
let signature = key.sign(manifest.as_bytes(), &artifact);
println!("{}", key.public_key_b64());           // give this to Forge admins
```

A platform administrator trusts the publisher in `forge.toml` and restarts
Forge:

```toml
[[trusted_publishers]]
publisher = "acme"
public_key = "<base64url public key>"
```

## 5. Install, enable and invoke

Install through the API, with the artifact in base64. Approve only the
permissions the plugin needs; the approved set is a subset of the requested
one.

```http
POST /api/v1/plugins
{"manifest_toml": "...", "artifact_base64": "...", "signature": "...",
 "approved_permissions": ["host:log", "host:server-online"]}

POST /api/v1/plugins/acme.uptime-probe/enable
POST /api/v1/plugins/acme.uptime-probe/invoke   {"scope": "workspace:<id>", "input": 0}
→ {"output": 1, "fuel_used": 1234}
```

**Administration → Plugins** lists installed plugins with their state and
fault count, and offers **Enable**, **Disable** and pinning. A quarantined
plugin shows **Release** instead of **Enable**.

## Verify

- **Administration → Plugins** lists the plugin as `enabled`.
- The invoke call returns an `output` and `fuel_used`.
- **Operations → Audit log** shows the install and enable.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| Install fails with a signature error | The publisher is not in `trusted_publishers`, or the manifest or artifact changed after signing | Add the publisher's key and restart; sign the exact bytes you upload |
| The component fails to instantiate | It imports WASI | Build for `wasm32-unknown-unknown` without WASI |
| A host call returns `false` or is refused | The permission was not approved, or the invoking user cannot `view` that server | Approve the permission; check the user's roles |
| The plugin is quarantined | It exceeded a budget, faulted too often, or its artifact on disk no longer matches the installed digest | Fix the plugin or raise `[budgets]`, upgrade it, then select **Release** |

## Next steps

- [Plugin manifest reference](/docs/reference/plugin-manifest/)
- [Plugins concept](/docs/concepts/plugins/)
- [REST API](/docs/reference/rest-api/) for `/plugins` endpoints.
