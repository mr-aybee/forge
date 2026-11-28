// A cluster's network plan and per-node sync state (`GET /clusters/{id}/fabric`).
// Server: `src/forge/crates/apps/forge-server/src/api/fabric.rs`.
import { useQuery } from "@tanstack/react-query";

import type { FabricResponse } from "../components/fabric/FabricForm";
import { get } from "./client";

const enc = encodeURIComponent;

/** Query keys of cluster network plans. */
export const fabricKeys = {
  one: (clusterId: string | null | undefined) => ["fabric", clusterId] as const,
};

/**
 * A cluster's network plan (cached; shared by node, cluster, app and stack
 * pages). `follow` polls every 5 s while the reconciler applies the plan.
 */
export const useClusterFabric = (clusterId?: string | null, opts?: { follow?: boolean }) =>
  useQuery({
    queryKey: fabricKeys.one(clusterId),
    queryFn: () => get<FabricResponse>(`/api/v1/clusters/${enc(clusterId ?? "")}/fabric`),
    enabled: !!clusterId,
    staleTime: 30_000,
    refetchInterval: opts?.follow ? (query) => (query.state.data?.status?.applied_by ? 5_000 : false) : false,
  });
