import { Loader2, Lock, Network, Plus } from "lucide-react";

import { useCreateSharedNetwork, useSwarmNetworks } from "../../api/swarm";
import { errorText, useToast } from "../../app/toast";
import { Badge, Button, Empty, ErrorNote, Panel, Skeleton, Table, Td } from "../ui";

export const SHARED_NETWORK = "forge-shared";

/** Overlay networks of the swarm: the routing mesh, `forge-shared` and stack networks. */
export function SwarmNetworks({ clusterId }: { clusterId: string }) {
  const q = useSwarmNetworks(clusterId);
  const create = useCreateSharedNetwork(clusterId);
  const toast = useToast();
  const list = q.data ?? [];
  const hasShared = list.some((n) => n.name === SHARED_NETWORK);
  const createBtn = (
    <Button
      variant={hasShared ? "default" : "primary"}
      disabled={hasShared || create.isPending || !!q.error}
      title={hasShared ? `${SHARED_NETWORK} exists` : "An attachable, encrypted overlay that services of different stacks join to talk to each other"}
      onClick={() => create.mutate(undefined, { onSuccess: () => toast(`Created ${SHARED_NETWORK}`), onError: (e) => toast(errorText(e), "error") })}
    >
      {create.isPending ? <Loader2 className="size-4 animate-spin" /> : <Plus className="size-4" />} Create {SHARED_NETWORK}
    </Button>
  );
  const sorted = [...list].sort((a, b) => rank(a) - rank(b) || a.name.localeCompare(b.name));

  return (
    <Panel title={`Networks${q.data ? ` · ${list.length}` : ""}`} description="Swarm-scoped overlays. The routing mesh publishes ports on every node." actions={createBtn}>
      {q.error ? (
        <div className="p-3">
          <ErrorNote error={q.error} />
        </div>
      ) : q.isLoading ? (
        <div className="p-3">
          <Skeleton rows={3} />
        </div>
      ) : list.length === 0 ? (
        <Empty icon={<Network className="size-5" />} title="No swarm networks" action={createBtn}>
          A manager reports no overlay networks yet. Deploying a stack creates its own; create {SHARED_NETWORK} to connect services across stacks.
        </Empty>
      ) : (
        <Table head={["Name", "Driver", "Subnets", "Flags", "Services"]}>
          {sorted.map((n) => (
            <tr key={n.id} className={`text-[13px] ${n.ingress || n.name === SHARED_NETWORK ? "bg-accent-soft/40" : ""}`}>
              <Td className="max-w-56">
                <div className="flex min-w-0 items-center gap-1.5">
                  <span className="truncate font-mono text-xs select-all" title={n.id}>
                    {n.name}
                  </span>
                  {n.ingress && <Badge tone="accent">routing mesh</Badge>}
                  {n.name === SHARED_NETWORK && <Badge tone="accent">shared</Badge>}
                  {n.managed && <Badge tone="muted">Forge</Badge>}
                </div>
              </Td>
              <Td className="text-xs text-muted">
                {n.driver || "—"}
                {n.scope && n.scope !== "swarm" ? ` · ${n.scope}` : ""}
              </Td>
              <Td className="font-mono text-[11px] select-all">{n.subnets.length ? n.subnets.join(", ") : "—"}</Td>
              <Td>
                <div className="flex flex-wrap gap-1">
                  {n.encrypted && (
                    <Badge tone="ok">
                      <Lock className="inline size-3" /> encrypted
                    </Badge>
                  )}
                  {n.attachable && <Badge tone="muted">attachable</Badge>}
                  {!n.encrypted && !n.attachable && <span className="text-faint">—</span>}
                </div>
              </Td>
              <Td className="max-w-72 truncate text-xs text-muted" title={n.services.join(", ")}>
                {n.services.length ? n.services.join(", ") : "—"}
              </Td>
            </tr>
          ))}
        </Table>
      )}
    </Panel>
  );
}

const rank = (n: { ingress: boolean; name: string }) => (n.ingress ? 0 : n.name === SHARED_NETWORK ? 1 : 2);
