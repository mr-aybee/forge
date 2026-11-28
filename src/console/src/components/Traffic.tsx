import { useQuery } from "@tanstack/react-query";
import { ArrowDown, ArrowUp } from "lucide-react";

import { get } from "../api/client";

export interface Flow {
  requests: number;
  in_bps: number;
  out_bps: number;
}
export interface NodeFlow {
  requests: number;
  /** Edges → this node (requests), bytes/s. */
  to_bps: number;
  /** This node → edges (responses), bytes/s. */
  from_bps: number;
}
export interface TrafficReport {
  window_secs: number;
  total: Flow;
  edges: Record<string, Flow & { sites: Record<string, Flow> }>;
  nodes: Record<string, NodeFlow>;
  unmatched: Record<string, NodeFlow>;
}

/** Route throughput through the edges (last minute), refreshed every 15 s. */
export const useTraffic = (clusterId?: string | null) =>
  useQuery({
    queryKey: ["traffic", clusterId ?? "all"],
    // Always a complete report: an older control plane (no /traffic yet) or a
    // failed request shows "no traffic" instead of breaking the page.
    queryFn: async (): Promise<TrafficReport> => {
      const r = await get<Partial<TrafficReport>>(`/api/v1/traffic${clusterId ? `?cluster_id=${encodeURIComponent(clusterId)}` : ""}`).catch(() => ({}) as Partial<TrafficReport>);
      const ok = r && typeof r === "object";
      return {
        window_secs: (ok && r.window_secs) || 60,
        total: { requests: 0, in_bps: 0, out_bps: 0, ...(ok ? r.total : undefined) },
        edges: (ok && r.edges) || {},
        nodes: (ok && r.nodes) || {},
        unmatched: (ok && r.unmatched) || {},
      };
    },
    refetchInterval: 15_000,
    staleTime: 10_000,
  });

/** Bytes per second: 812 B/s, 12.4 KB/s, 3.1 MB/s, 1.2 GB/s. */
export function bytesRate(bps: number): string {
  if (!bps || bps < 1) return "0 B/s";
  if (bps < 1024) return `${Math.round(bps)} B/s`;
  const kb = bps / 1024;
  if (kb < 1024) return `${kb.toFixed(kb < 10 ? 1 : 0)} KB/s`;
  const mb = kb / 1024;
  if (mb < 1024) return `${mb.toFixed(mb < 10 ? 1 : 0)} MB/s`;
  return `${(mb / 1024).toFixed(1)} GB/s`;
}

/** Compact "↓ to · ↑ from" cell for a node (what edges send it / get back). */
export function NodeTrafficCell({ flow }: { flow?: NodeFlow }) {
  if (!flow || flow.to_bps + flow.from_bps === 0) return <span className="text-xs text-faint">—</span>;
  return (
    <span className="inline-flex flex-col font-mono text-[11px] leading-tight tabular-nums" title={`Through the edges, last minute: ${flow.requests} requests · to this node ${bytesRate(flow.to_bps)} · back ${bytesRate(flow.from_bps)}`}>
      <span className="inline-flex items-center gap-0.5 text-accent">
        <ArrowDown className="size-3" />
        {bytesRate(flow.to_bps)}
      </span>
      <span className="inline-flex items-center gap-0.5 text-muted">
        <ArrowUp className="size-3" />
        {bytesRate(flow.from_bps)}
      </span>
    </span>
  );
}
