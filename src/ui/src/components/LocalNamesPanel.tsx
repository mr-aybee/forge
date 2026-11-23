import { Globe } from "lucide-react";
import { useMemo, useState } from "react";

import { useLocalNames } from "../api/hooks";
import type { LocalName } from "../api/types";
import { NameSuffixEditor } from "./NameSuffixEditor";
import { Pager, usePaged } from "./Pager";
import { Badge, Empty, Input, Mono, Panel, Skeleton, Table, Td } from "./ui";

const ports = (n: LocalName) => (n.ports ?? []).map((p) => `${p.port}/${p.proto} → ${p.target}${p.bind ? ` (${p.bind})` : ""}`).join(", ");

/**
 * The fleet's local names (D-084): every node writes these into its hosts
 * file, so `web-1.local` or `api.shop.prod.local` resolve on every node and
 * inside its Forge containers. Ports are the ones the service publishes.
 */
export function LocalNamesPanel() {
  const q = useLocalNames();
  const [filter, setFilter] = useState("");
  const rows = useMemo(() => {
    const f = filter.trim().toLowerCase();
    return (q.data?.names ?? []).filter((n) => !f || n.name.includes(f) || (n.app ?? "").toLowerCase().includes(f));
  }, [q.data, filter]);
  const paged = usePaged(rows, { reset: filter.trim().toLowerCase() });
  const sync = q.data?.nodes ?? [];
  const behind = sync.filter((n) => n.sync && !n.sync.in_sync).length;
  return (
    <Panel
      title="Local names"
      description={
        q.data
          ? q.data.enabled
            ? `Names ending in .${q.data.suffix}, kept in every node's hosts file and in its Forge containers. ${behind ? `${behind} node(s) not updated yet.` : "All nodes are up to date."}`
            : "Turned off on this server ([names] enabled = false)."
          : undefined
      }
      actions={<Input className="w-56" placeholder="Filter names" value={filter} onChange={(e) => setFilter(e.target.value)} />}
    >
      <NameSuffixEditor example={sync.find((n) => n.label)?.label ?? undefined} />
      {q.isLoading ? (
        <Skeleton rows={3} />
      ) : rows.length === 0 ? (
        <Empty icon={<Globe className="size-5" />} title="No names yet">
          Enroll a node; its name appears here and on every other node.
        </Empty>
      ) : (
        <>
        <Table head={["Name", "Kind", "Node", "Address", "Mesh", "Ports"]}>
          {paged.rows.map((n) => {
            const s = sync.find((x) => x.server_id === n.server_id)?.sync;
            return (
              <tr key={`${n.name}-${n.server_id}`}>
                <Td>
                  <Mono>{n.name}</Mono>
                </Td>
                <Td>
                  <Badge tone={n.kind === "node" ? "accent" : "ok"}>{n.kind}</Badge>
                  {n.app && <span className="ml-2 text-xs text-muted">{n.app}</span>}
                </Td>
                <Td>
                  {n.node}
                  {s?.error && (
                    <span className="ml-2 text-xs text-bad" title={s.error}>
                      not updated
                    </span>
                  )}
                </Td>
                <Td>
                  <Mono>{n.address ?? "—"}</Mono>
                </Td>
                <Td>
                  <Mono>{n.mesh_address ?? "—"}</Mono>
                </Td>
                <Td className="text-xs">{ports(n) || "—"}</Td>
              </tr>
            );
          })}
        </Table>
        <Pager paged={paged} />
        </>
      )}
    </Panel>
  );
}
