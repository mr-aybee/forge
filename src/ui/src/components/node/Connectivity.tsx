import { Link } from "@tanstack/react-router";
import { ArrowRight, Network, Play } from "lucide-react";
import { useEffect, useState } from "react";

import { useConnectivity, useRunProbe, type ProbeResult } from "../../api/hooks";
import type { Server } from "../../api/types";
import { errorText, useToast } from "../../app/toast";
import { Sparkline } from "../charts/TimeSeries";
import { Badge, Button, Empty, ErrorNote, Select, Table, Td, ago } from "../ui";

const AUTO = [
  { label: "Manual", ms: 0 },
  { label: "Every 30 s", ms: 30_000 },
  { label: "Every 1 min", ms: 60_000 },
  { label: "Every 5 min", ms: 300_000 },
];

function latencyTone(ms?: number | null) {
  if (ms === undefined || ms === null) return "";
  return ms > 100 ? "text-bad" : ms > 20 ? "text-warn" : "text-ok";
}

/**
 * Probes every peer (other cluster members and the control node) from the
 * selected node, via its agent: ICMP ping, or a TCP connect to the peer's SSH
 * port when ping is unavailable or filtered.
 */
export function Connectivity({ node }: { node: Server }) {
  const toast = useToast();
  const data = useConnectivity(node.id);
  const run = useRunProbe(node.id);
  const [count, setCount] = useState(4);
  const [auto, setAuto] = useState(0);
  const online = node.status === "online";
  const peers = data.data?.peers ?? [];
  const runs = data.data?.runs ?? [];
  const lastRun = runs[runs.length - 1];

  const go = () =>
    run.mutate(count, {
      onSuccess: (r) => {
        if (r.status !== "succeeded") toast(r.error ?? "The check did not finish in time; results will appear when it does.", "error");
      },
      onError: (e) => toast(errorText(e), "error"),
    });

  useEffect(() => {
    if (!auto || !online || peers.length === 0) return;
    const id = setInterval(() => {
      if (!run.isPending) go();
    }, auto);
    return () => clearInterval(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [auto, online, peers.length]);

  const resultFor = (serverId: string): ProbeResult | undefined => lastRun?.results.find((r) => r.server_id === serverId);
  const trend = (serverId: string) => runs.slice(-30).map((r) => r.results.find((x) => x.server_id === serverId)?.latency_ms ?? null);

  return (
    <div>
      <div className="flex flex-wrap items-center gap-2 border-b border-line px-4 py-2.5 text-xs text-muted">
        <span>{lastRun ? `Last check ${ago(lastRun.t)}` : "Not checked yet"}</span>
        <div className="ml-auto flex flex-wrap items-center gap-2">
          <label className="flex items-center gap-1.5">
            Packets
            <Select className="w-auto py-1 text-xs" value={count} onChange={(e) => setCount(Number(e.target.value))}>
              {[1, 3, 4, 5, 10].map((c) => (
                <option key={c} value={c}>
                  {c}
                </option>
              ))}
            </Select>
          </label>
          <label className="flex items-center gap-1.5">
            Repeat
            <Select className="w-auto py-1 text-xs" value={auto} onChange={(e) => setAuto(Number(e.target.value))}>
              {AUTO.map((a) => (
                <option key={a.ms} value={a.ms}>
                  {a.label}
                </option>
              ))}
            </Select>
          </label>
          <Button variant="primary" disabled={!online || peers.length === 0 || run.isPending} onClick={go} title={online ? undefined : "The agent must be online"}>
            <Play className="size-4" /> {run.isPending ? "Checking…" : "Run checks"}
          </Button>
        </div>
      </div>
      <ErrorNote error={data.error} />
      {peers.length === 0 ? (
        <Empty icon={<Network className="size-5" />} title="No peers to check">
          {node.cluster_id ? "This is the only node in its cluster." : "Add this node to a cluster to check its connectivity to the other members."}
        </Empty>
      ) : (
        <Table head={["Source → destination", "Target", "Status", "Latency", "Loss", "Method", "Trend"]}>
          {peers.map((p) => {
            const r = resultFor(p.server_id);
            return (
              <tr key={p.server_id} className="hover:bg-panel-2/60">
                <Td>
                  <span className="flex items-center gap-1.5 whitespace-nowrap">
                    <span className="text-muted">{node.hostname}</span>
                    <ArrowRight className="size-3.5 text-faint" />
                    <Link to="/nodes/$nodeId" params={{ nodeId: p.server_id }} className="font-medium hover:text-accent">
                      {p.hostname}
                    </Link>
                    <Badge tone={p.role === "control" ? "accent" : "muted"}>{p.role === "control" ? "Control" : "Worker"}</Badge>
                  </span>
                </Td>
                <Td className="font-mono text-xs text-muted">{p.target}</Td>
                <Td>
                  {!r ? (
                    <span className="text-faint">—</span>
                  ) : r.reachable ? (
                    <Badge tone="ok">reachable</Badge>
                  ) : (
                    <span title={r.error}>
                      <Badge tone="bad">unreachable</Badge>
                    </span>
                  )}
                </Td>
                <Td className={`tabular-nums ${latencyTone(r?.latency_ms)}`}>{r?.latency_ms !== undefined && r.latency_ms !== null ? `${r.latency_ms} ms` : "—"}</Td>
                <Td className={`tabular-nums ${r && (r.loss_percent ?? 0) > 0 ? "text-warn" : ""}`}>{r?.loss_percent !== undefined ? `${r.loss_percent}%` : "—"}</Td>
                <Td className="text-xs text-muted">{r?.method === "tcp" ? "TCP connect" : r?.method === "icmp" ? "ICMP" : "—"}</Td>
                <Td>
                  <Sparkline values={trend(p.server_id)} />
                </Td>
              </tr>
            );
          })}
        </Table>
      )}
    </div>
  );
}
