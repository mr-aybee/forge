//! Plugin host.
//!
//! * [`PluginHost`] — lifecycle: install → enable ⇄ disable → upgrade / pin →
//!   uninstall, plus quarantine. Every mutation is authorized (`plugin.manage`)
//!   and audited.
//! * [`PluginRunner`] — port for executing a component; [`ProcessRunner`]
//!   spawns `forge-plugin-runner` per invocation (zero cost when idle).
//! * [`HostCalls`] / [`ForgeHostFunctions`] — typed host functions. Effective
//!   permission = invoking user (Cedar) ∩ plugin's approved permissions ∩
//!   resource policy (Cedar guardrails).

mod functions;
mod host;
mod runner;

pub use functions::{ForgeHostFunctions, Invocation};
pub use host::{InvokeOutcome, PluginHost, PluginHostConfig, PluginPackage};
pub use runner::{HostCalls, PluginRunner, ProcessRunner};
