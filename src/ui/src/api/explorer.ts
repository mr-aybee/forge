import { useQuery } from "@tanstack/react-query";
import { useMemo } from "react";

import { buildTree, type CheckStatus, type HealthMap, type TreeProject } from "../lib/explorerTree";
import { get } from "./client";
import { useAllEnvironments, useClusters, useProjects, useServices, useServers } from "./hooks";
import type { HealthSummaryList } from "./types";

/** Query keys of the Project Explorer. */
export const explorerKeys = { all: ["explorer"] as const, healthSummary: () => [...explorerKeys.all, "health-summary"] as const };

/** Normalizes the summary; `items: []` when the server is older or the request fails. */
function normSummary(r: unknown): HealthSummaryList {
  const o = (r && typeof r === "object" ? r : {}) as Partial<HealthSummaryList>;
  const items = Array.isArray(o.items) ? o.items : [];
  return {
    truncated: o.truncated === true,
    items: items
      .filter((x) => x && typeof x.service_id === "string")
      .map((x) => ({ service_id: x.service_id, instances: Array.isArray(x.instances) ? x.instances.filter((i) => i && typeof i.server_id === "string") : [] })),
  };
}

/** Per-node health of every visible service in one request; refreshed every 15 s like the health tab. */
export const useHealthSummary = () =>
  useQuery({
    queryKey: explorerKeys.healthSummary(),
    queryFn: () => get<unknown>(`/api/v1/services/health-summary`).then(normSummary),
    refetchInterval: 15_000,
    staleTime: 10_000,
  });

function toHealthMap(list: HealthSummaryList | undefined): HealthMap {
  const m = new Map<string, Map<string, CheckStatus>>();
  for (const a of list?.items ?? []) m.set(a.service_id, new Map(a.instances.map((i) => [i.server_id, i.status])));
  return m;
}

/** The Project Explorer tree, joined in the browser from list queries (no per-row requests). */
export function useExplorerTree(): { tree: TreeProject[]; isLoading: boolean; error: unknown; healthTruncated: boolean; healthError: unknown } {
  const projects = useProjects();
  const environments = useAllEnvironments();
  const services = useServices();
  const servers = useServers();
  const health = useHealthSummary();
  const clusters = useClusters();
  const tree = useMemo(() => {
    const swarm = new Set((clusters.data ?? []).filter((c) => c.mode === "swarm").map((c) => c.id));
    return buildTree(projects.data ?? [], environments.data ?? [], services.data ?? [], servers.data ?? [], toHealthMap(health.data), swarm);
  }, [projects.data, environments.data, services.data, servers.data, health.data, clusters.data]);
  return {
    tree,
    isLoading: projects.isLoading || environments.isLoading || services.isLoading || servers.isLoading,
    // A failed health summary is not fatal: the tree falls back to the state nodes report.
    error: projects.error ?? environments.error ?? services.error ?? servers.error,
    healthTruncated: !!health.data?.truncated,
    healthError: health.error,
  };
}
