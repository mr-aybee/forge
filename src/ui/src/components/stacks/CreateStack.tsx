import { useAllEnvironments, useClusters, useServers } from "../../api/hooks";

/** Environments offered for a node: those of its cluster (all when it has none). */
export function useEnvironmentChoices(serverId: string) {
  const environments = useAllEnvironments().data ?? [];
  const clusters = useClusters().data ?? [];
  const server = (useServers().data ?? []).find((s) => s.id === serverId);
  const list = server?.cluster_id ? environments.filter((n) => n.cluster_id === server.cluster_id) : environments;
  const clusterName = (id: string | null) => clusters.find((c) => c.id === id)?.name ?? "Unassigned";
  return { list: list.length ? list : environments, server, clusterName };
}
