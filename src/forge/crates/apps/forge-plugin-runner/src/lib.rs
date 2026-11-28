//! Executes one plugin invocation inside Wasmtime with strict budgets.
//!
//! * **No WASI.** The linker defines only `forge:plugin/host`; a component
//!   importing anything else fails to instantiate.
//! * **Fuel** bounds instructions; **epoch deadlines** bound wall time;
//!   **StoreLimits** bound memory (growth beyond the limit traps); host-call
//!   output (logs, arguments) is capped.
//! * Host functions are forwarded to the host over a [`HostChannel`]; the
//!   host decides (Cedar + approved permissions) and may refuse.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use forge_sdk::runner_protocol::{FailureKind, RunRequest, RunnerMessage};
use forge_sdk::{HOST_INTERFACE, host_functions};
use serde_json::{Value, json};
use wasmtime::component::{Component, Linker};
use wasmtime::{Config, Engine, ResourceLimiter, Store, StoreContextMut, StoreLimits, StoreLimitsBuilder, Trap};

/// Epoch tick length used for wall-clock deadlines.
const EPOCH_TICK: Duration = Duration::from_millis(10);

/// Channel to the host for typed host calls.
pub trait HostChannel: Send {
    fn call(&mut self, function: &str, args: Value) -> Result<Value, String>;
}

/// Store limits with the memory budget as a TOTAL across every memory of the
/// plugin (`StoreLimits` applies its size to each memory on its own, so a
/// component with several memories could use a multiple of the budget).
struct Limits {
    inner: StoreLimits,
    total: usize,
    used: usize,
}

impl ResourceLimiter for Limits {
    fn memory_growing(&mut self, current: usize, desired: usize, maximum: Option<usize>) -> wasmtime::Result<bool> {
        let grow = desired.saturating_sub(current);
        if self.used.saturating_add(grow) > self.total {
            wasmtime::bail!("forcing trap when growing memory to {desired} bytes: total budget {} bytes", self.total);
        }

        let ok = self.inner.memory_growing(current, desired, maximum)?;
        if ok {
            self.used = self.used.saturating_add(grow);
        }

        Ok(ok)
    }

    fn memory_grow_failed(&mut self, error: wasmtime::Error) -> wasmtime::Result<()> {
        self.inner.memory_grow_failed(error)
    }

    fn table_growing(&mut self, current: usize, desired: usize, maximum: Option<usize>) -> wasmtime::Result<bool> {
        self.inner.table_growing(current, desired, maximum)
    }

    fn table_grow_failed(&mut self, error: wasmtime::Error) -> wasmtime::Result<()> {
        self.inner.table_grow_failed(error)
    }

    fn instances(&self) -> usize {
        self.inner.instances()
    }

    fn tables(&self) -> usize {
        self.inner.tables()
    }

    fn memories(&self) -> usize {
        self.inner.memories()
    }
}

/// Elements per table (function references); generous for real plugins.
const MAX_TABLE_ELEMENTS: usize = 100_000;

struct State {
    limits: Limits,
    channel: Box<dyn HostChannel>,
    output_left: u64,
    denied: Option<String>,
    output_exceeded: bool,
}

impl State {
    fn charge_output(&mut self, n: usize) -> wasmtime::Result<()> {
        let n = u64::try_from(n).unwrap_or(u64::MAX);
        if n > self.output_left {
            self.output_exceeded = true;
            wasmtime::bail!("output budget exceeded");
        }

        self.output_left -= n;

        Ok(())
    }

    fn forward(&mut self, function: &str, args: Value) -> wasmtime::Result<Value> {
        match self.channel.call(function, args) {
            Ok(v) => Ok(v),
            Err(e) => {
                self.denied = Some(e.clone());
                wasmtime::bail!("host refused `{function}`: {e}")
            }
        }
    }
}

fn engine() -> wasmtime::Result<Engine> {
    let mut config = Config::new();
    config.wasm_component_model(true);
    config.consume_fuel(true);
    config.epoch_interruption(true);

    Engine::new(&config)
}

fn linker(engine: &Engine) -> wasmtime::Result<Linker<State>> {
    let mut linker = Linker::<State>::new(engine);
    let mut host = linker.instance(HOST_INTERFACE)?;
    host.func_wrap(host_functions::LOG, |mut ctx: StoreContextMut<'_, State>, (message,): (String,)| {
        let state = ctx.data_mut();
        state.charge_output(message.len())?;
        state.forward(host_functions::LOG, json!({ "message": message }))?;
        Ok(())
    })?;
    host.func_wrap(host_functions::SERVER_ONLINE, |mut ctx: StoreContextMut<'_, State>, (hostname,): (String,)| {
        let state = ctx.data_mut();
        state.charge_output(hostname.len())?;
        let v = state.forward(host_functions::SERVER_ONLINE, json!({ "hostname": hostname }))?;
        Ok((v.as_bool().unwrap_or(false),))
    })?;

    Ok(linker)
}

/// Runs the component and returns the terminal message (`Done` or `Failed`).
pub fn execute(req: &RunRequest, component_bytes: &[u8], channel: Box<dyn HostChannel>) -> RunnerMessage {
    match execute_inner(req, component_bytes, channel) {
        Ok(msg) => msg,
        Err(e) => RunnerMessage::Failed { kind: FailureKind::Invalid, message: format!("{e:#}") },
    }
}

fn execute_inner(req: &RunRequest, bytes: &[u8], channel: Box<dyn HostChannel>) -> wasmtime::Result<RunnerMessage> {
    let engine = engine()?;
    let component = Component::new(&engine, bytes)?;
    let linker = linker(&engine)?;

    let memory = usize::try_from(req.memory_bytes).unwrap_or(usize::MAX);
    let inner = StoreLimitsBuilder::new()
        .memory_size(memory)
        .instances(16)
        .tables(16)
        .table_elements(MAX_TABLE_ELEMENTS)
        .memories(4)
        .trap_on_grow_failure(true)
        .build();
    let limits = Limits { inner, total: memory, used: 0 };
    let state =
        State { limits, channel, output_left: u64::from(req.max_output_bytes), denied: None, output_exceeded: false };
    let mut store = Store::new(&engine, state);
    store.limiter(|s| &mut s.limits);
    store.set_fuel(req.fuel)?;
    let ticks = (req.timeout_ms / u64::try_from(EPOCH_TICK.as_millis()).unwrap_or(10)).max(1);
    store.set_epoch_deadline(ticks);
    store.epoch_deadline_trap();

    // Wall-clock ticker for epoch interruption.
    let done = Arc::new(AtomicBool::new(false));

    let ticker = {
        let engine = engine.clone();
        let done = done.clone();
        std::thread::spawn(move || {
            while !done.load(Ordering::Relaxed) {
                std::thread::sleep(EPOCH_TICK);
                engine.increment_epoch();
            }
        })
    };

    let result = (|| -> wasmtime::Result<u32> {
        let instance = linker.instantiate(&mut store, &component)?;
        let run = instance.get_typed_func::<(u32,), (u32,)>(&mut store, &req.export)?;
        let (out,) = run.call(&mut store, (req.input,))?;
        Ok(out)
    })();

    done.store(true, Ordering::Relaxed);
    let _ = ticker.join();

    let fuel_used = req.fuel.saturating_sub(store.get_fuel().unwrap_or(0));

    Ok(match result {
        Ok(output) => RunnerMessage::Done { output, fuel_used },
        Err(e) => {
            let (kind, message) = classify(&e, store.data());
            RunnerMessage::Failed { kind, message }
        }
    })
}

fn classify(err: &wasmtime::Error, state: &State) -> (FailureKind, String) {
    if let Some(reason) = &state.denied {
        return (FailureKind::Denied, reason.clone());
    }

    if state.output_exceeded {
        return (FailureKind::OutputExceeded, "output budget exceeded".into());
    }

    if let Some(trap) = err.downcast_ref::<Trap>() {
        let kind = match trap {
            Trap::OutOfFuel => FailureKind::FuelExhausted,
            Trap::Interrupt => FailureKind::Timeout,
            _ => FailureKind::Trap,
        };

        return (kind, trap.to_string());
    }

    let text = format!("{err:#}");
    if text.contains("growing memory") || text.contains("memory limits") || text.contains("exceeds memory") {
        return (FailureKind::MemoryExceeded, text);
    }

    (FailureKind::Trap, text)
}
