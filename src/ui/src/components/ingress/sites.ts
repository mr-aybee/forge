import { useQuery } from "@tanstack/react-query";

import { get } from "../../api/client";
import type { ServiceInstance } from "../../api/types";
import type { Site, Target } from "./types";

/**
 * Every endpoint (`GET /api/v1/sites`), normalised so callers never index a
 * missing field: `config`, `spec`, `edges`, `targets` and `standby_targets`
 * always exist. A failed request yields an empty list and `error`.
 */
export function useSites(enabled = true) {
  return useQuery({
    queryKey: ["sites"],
    enabled,
    queryFn: async (): Promise<Site[]> => {
      const raw = await get<unknown>("/api/v1/sites");
      return (Array.isArray(raw) ? raw : []).map(normSite).filter((s): s is Site => !!s);
    },
    retry: false,
  });
}

const normSite = (v: unknown): Site | undefined => {
  if (!v || typeof v !== "object") return undefined;
  const s = v as Partial<Site> & { config?: Partial<Site["config"]> };
  if (typeof s.id !== "string") return undefined;
  const cfg: Partial<Site["config"]> = s.config ?? {};
  const spec = { ...(cfg.spec ?? {}) } as Site["config"]["spec"];
  spec.domains = Array.isArray(spec.domains) ? spec.domains : [];
  spec.upstreams = Array.isArray(spec.upstreams) ? spec.upstreams : [];
  spec.standby = Array.isArray(spec.standby) ? spec.standby : [];
  spec.balance = spec.balance ?? "round_robin";
  spec.tls = spec.tls ?? "off";
  spec.custom = typeof spec.custom === "string" ? spec.custom : "";
  return {
    id: s.id,
    name: s.name ?? s.id.slice(0, 8),
    config: {
      spec,
      edges: Array.isArray(cfg.edges) ? cfg.edges : [],
      dns: cfg.dns ?? null,
      cluster_id: cfg.cluster_id ?? null,
      targets: Array.isArray(cfg.targets) ? cfg.targets : [],
      standby_targets: Array.isArray(cfg.standby_targets) ? cfg.standby_targets : [],
    },
    edge_status: s.edge_status && typeof s.edge_status === "object" ? s.edge_status : {},
    updated_at: typeof s.updated_at === "number" ? s.updated_at : 0,
  };
};

const targetsApp = (list: Target[] | undefined, appId: string) => (list ?? []).some((t) => t?.kind === "app" && t.service_id === appId);

/** The endpoints that route to service `appId`, as an active or a standby target. */
export const sitesForApp = (sites: Site[] | undefined, appId: string | undefined): Site[] =>
  appId ? (sites ?? []).filter((s) => targetsApp(s.config?.targets, appId) || targetsApp(s.config?.standby_targets, appId)) : [];

/** How many replicas (active) and standbys a service's placements give an endpoint. */
export const fanOut = (instances: ServiceInstance[] | undefined) => {
  const list = Array.isArray(instances) ? instances : [];
  return { replicas: list.filter((i) => i.role !== "standby").length, standbys: list.filter((i) => i.role === "standby").length };
};
