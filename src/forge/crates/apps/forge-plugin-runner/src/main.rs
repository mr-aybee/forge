//! `forge-plugin-runner`: reads one `RunRequest` from stdin, executes the
//! component, forwards host calls over stdio and prints the final message.

use std::io::{BufRead, BufReader, Stdin, Write};

use forge_plugin_runner::{HostChannel, execute};
use forge_sdk::runner_protocol::{FailureKind, HostReply, RunRequest, RunnerMessage};
use serde_json::Value;

fn send(msg: &RunnerMessage) -> std::io::Result<()> {
    let line = serde_json::to_string(msg).map_err(std::io::Error::other)?;
    let mut out = std::io::stdout().lock();
    out.write_all(line.as_bytes())?;
    out.write_all(b"\n")?;

    out.flush()
}

/// Host calls over stdio: one request line out, one reply line in.
struct StdioChannel {
    reader: BufReader<Stdin>,
    next_id: u64,
}

impl HostChannel for StdioChannel {
    fn call(&mut self, function: &str, args: Value) -> Result<Value, String> {
        self.next_id += 1;
        let id = self.next_id;
        send(&RunnerMessage::HostCall { id, function: function.to_owned(), args }).map_err(|e| e.to_string())?;
        let mut line = String::new();
        self.reader.read_line(&mut line).map_err(|e| e.to_string())?;
        let reply: HostReply = serde_json::from_str(&line).map_err(|e| e.to_string())?;
        if reply.id != id {
            return Err("host reply out of order".into());
        }

        match reply.error {
            Some(err) => Err(err),
            None => Ok(reply.result.unwrap_or(Value::Null)),
        }
    }
}

fn run() -> Result<RunnerMessage, String> {
    let mut reader = BufReader::new(std::io::stdin());
    let mut first = String::new();
    reader.read_line(&mut first).map_err(|e| format!("no request on stdin: {e}"))?;
    let req: RunRequest = serde_json::from_str(&first).map_err(|e| format!("bad request: {e}"))?;
    let bytes = std::fs::read(&req.artifact_path).map_err(|e| format!("cannot read artifact: {e}"))?;
    req.verify_artifact(&bytes)?;

    Ok(execute(&req, &bytes, Box::new(StdioChannel { reader, next_id: 0 })))
}

fn main() {
    let msg = run().unwrap_or_else(|message| RunnerMessage::Failed { kind: FailureKind::Invalid, message });
    if send(&msg).is_err() {
        std::process::exit(2);
    }
}
