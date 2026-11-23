// Docker container telemetry of a node: metrics history and collection settings.

import { useQuery } from "@tanstack/react-query";

import { analyticsUnavailable } from "./analytics";
import { api, get } from "./client";
import { useAction } from "./hooks";

const enc = encodeURIComponent;

export interface ContainerPoint {
  /** Epoch ms, start of the minute or hour. */
  t: number;
  /** Percent of one core (can exceed 100 on several cores). */
  cpu_avg: number;
  cpu_max: number;
  /** Bytes. */
  mem_used: number;
  mem_limit: number;
  /** Average bytes per second. */
  rx: number;
  tx: number;
  block_read: number;
  block_write: number;
  restarts: number;
}

export interface ContainerSeries {
  container_name: string;
  container_id: string;
  image: string;
  service_id: string | null;
  stack: string | null;
  points: ContainerPoint[];
}

export interface ContainerHistory {
  server_id: string;
  from: number;
  to: number;
  resolution: "minute" | "hour";
  containers: ContainerSeries[];
}

export interface ContainerTelemetrySettings {
  enabled: boolean;
  /** Container names, or `key=value` label matches. */
  exclude: string[];
}

export const containerTelemetryKeys = {
  all: ["container-telemetry"] as const,
  metrics: (server: string, name: string | undefined, hours: number) => ["container-telemetry", "metrics", server, name ?? "*", hours] as const,
  settings: (server: string) => ["container-telemetry", "settings", server] as const,
};

const num = (v: unknown) => (typeof v === "number" && Number.isFinite(v) ? v : 0);
const optStr = (v: unknown) => (typeof v === "string" && v ? v : null);

function normHistory(r: Partial<ContainerHistory> | null | undefined, server: string): ContainerHistory {
  const list: Partial<ContainerSeries>[] = Array.isArray(r?.containers) ? r.containers : [];
  return {
    server_id: r?.server_id ?? server,
    from: num(r?.from),
    to: num(r?.to),
    resolution: r?.resolution === "hour" ? "hour" : "minute",
    containers: list
      .filter((c) => c && typeof c.container_name === "string")
      .map((c) => ({
        container_name: c.container_name ?? "",
        container_id: c.container_id ?? "",
        image: c.image ?? "",
        service_id: optStr(c.service_id),
        stack: optStr(c.stack),
        points: (Array.isArray(c.points) ? c.points : [])
          .filter((p) => p && typeof p.t === "number")
          .map((p) => ({
            t: p.t,
            cpu_avg: num(p.cpu_avg),
            cpu_max: num(p.cpu_max),
            mem_used: num(p.mem_used),
            mem_limit: num(p.mem_limit),
            rx: num(p.rx),
            tx: num(p.tx),
            block_read: num(p.block_read),
            block_write: num(p.block_write),
            restarts: num(p.restarts),
          })),
      })),
  };
}

/**
 * History of a node's containers (all, or the one named) over the last
 * `hours`. Refreshes every minute. 503 when analytics is off; not retried.
 */
export function useContainerMetrics(server: string, name: string | undefined, hours: number) {
  return useQuery({
    queryKey: containerTelemetryKeys.metrics(server, name, hours),
    queryFn: async () => {
      const to = Date.now();
      const q = `?from=${to - hours * 3_600_000}&to=${to}`;
      const path = name ? `/api/v1/servers/${enc(server)}/container-metrics/${enc(name)}${q}` : `/api/v1/servers/${enc(server)}/container-metrics${q}&limit=50`;
      return normHistory(await get<Partial<ContainerHistory>>(path), server);
    },
    enabled: !!server,
    refetchInterval: 60_000,
    staleTime: 30_000,
    retry: (n, e) => !analyticsUnavailable(e) && n < 2,
  });
}

/** Whether the node collects container logs and metrics, and what it leaves out. */
export function useContainerTelemetrySettings(server: string) {
  return useQuery({
    queryKey: containerTelemetryKeys.settings(server),
    queryFn: async (): Promise<ContainerTelemetrySettings> => {
      const r = await get<Partial<ContainerTelemetrySettings>>(`/api/v1/servers/${enc(server)}/container-telemetry`);
      return { enabled: r?.enabled !== false, exclude: Array.isArray(r?.exclude) ? r.exclude.filter((x): x is string => typeof x === "string") : [] };
    },
    enabled: !!server,
  });
}

export const useSaveContainerTelemetry = (server: string) =>
  useAction((b: ContainerTelemetrySettings) => api<ContainerTelemetrySettings>("PUT", `/api/v1/servers/${enc(server)}/container-telemetry`, b), [containerTelemetryKeys.settings(server)]);
