// Node history from the analytics database (D-127).

import { useQuery } from "@tanstack/react-query";

import { ApiError, get } from "./client";

export interface NodePoint {
  /** Start of the minute or hour, epoch ms. */
  t: number;
  cpu_avg: number;
  cpu_max: number;
  mem_used: number;
  mem_total: number;
  disk_used: number;
  disk_total: number;
  rx: number;
  tx: number;
  load1: number;
}

export interface NodeHistory {
  server_id: string;
  from: number;
  to: number;
  resolution: "minute" | "hour";
  points: NodePoint[];
}

export interface Peak {
  t: number;
  value: number;
}

export interface ProcessUsage {
  name: string;
  user: string;
  cpu_avg: number;
  cpu_max: number;
  mem_avg: number;
  mem_max: number;
  minutes: number;
  cmd: string;
}

export interface NodeReport extends NodeHistory {
  cpu_peak: Peak | null;
  mem_peak: Peak | null;
  disk_peak: Peak | null;
  top_cpu: ProcessUsage[];
  top_mem: ProcessUsage[];
}

export const analyticsKeys = {
  all: ["analytics"] as const,
  report: (server: string, hours: number) => ["analytics", "report", server, hours] as const,
};

/** Analytics is off or unreachable: an expected state, shown as a note. */
export function analyticsUnavailable(e: unknown): string | undefined {
  if (e instanceof ApiError && (e.code === "analytics_off" || e.code === "analytics_unavailable")) return e.message;
  return undefined;
}

/**
 * A node report for the last `hours` hours: history (minute points up to
 * 48 hours, hourly beyond), peaks and top processes. Refreshes every minute,
 * as new minutes are written once a minute.
 */
export function useNodeReport(server: string, hours: number, enabled = true) {
  return useQuery({
    queryKey: analyticsKeys.report(server, hours),
    enabled,
    refetchInterval: 60_000,
    retry: (n, e) => !analyticsUnavailable(e) && n < 2,
    queryFn: () => {
      const to = Date.now();
      return get<NodeReport>(`/api/v1/servers/${encodeURIComponent(server)}/report?from=${to - hours * 3_600_000}&to=${to}`);
    },
  });
}
