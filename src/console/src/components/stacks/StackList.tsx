import { Link } from "@tanstack/react-router";
import { Layers } from "lucide-react";
import type { ReactNode } from "react";

import { useAllEnvironments } from "../../api/hooks";
import type { StackEntry } from "../../api/stacks";
import { Badge, Empty, Table, Td, ago, time } from "../ui";
import { NodeLink, StateChip } from "./parts";

/** Stacks as a dense table: name → environment → nodes with their status. */
export function StackTable({ rows, empty, showEnvironment = true }: { rows: StackEntry[]; empty?: ReactNode; showEnvironment?: boolean }) {
  const environments = useAllEnvironments().data ?? [];
  const list = Array.isArray(rows) ? rows : [];
  if (!list.length)
    return (
      empty ?? (
        <Empty icon={<Layers className="size-5" />} title="No stacks">
          Create one from a compose file, or create a single container.
        </Empty>
      )
    );
  return (
    <Table head={["Stack", ...(showEnvironment ? ["Environment"] : []), "Nodes", "Revision", "Updated"]}>
      {list.map(({ stack: s, nodes }) => {
        const ns = environments.find((n) => n.id === s.environment_id);
        const running = nodes.reduce((a, n) => a + (n.state ?? []).filter((x) => x.state === "running").length, 0);
        const total = nodes.reduce((a, n) => a + (n.services?.length ?? 0), 0);
        return (
          <tr key={s.id} className="hover:bg-panel-2/60">
            <Td className="max-w-[260px]">
              <div className="flex min-w-0 items-center gap-2">
                <Link to="/stacks/$stackId" params={{ stackId: s.id }} className="truncate font-medium hover:text-accent" title={s.name}>
                  {s.name}
                </Link>
                {s.kind === "container" && <Badge>container</Badge>}
              </div>
              <div className="font-mono text-[11px] text-faint tabular-nums">
                {total ? `${running}/${total} running` : "not deployed"}
                {s.env_keys.length > 0 && ` · ${s.env_keys.length} env`}
              </div>
            </Td>
            {showEnvironment && (
              <Td>
                {ns ? (
                  <Link to="/stacks" search={{ ns: ns.id }} className="text-xs hover:text-accent">
                    <Badge tone="accent">{ns.name}</Badge>
                  </Link>
                ) : (
                  <span className="text-faint">—</span>
                )}
              </Td>
            )}
            <Td>
              {nodes.length === 0 ? (
                <span className="text-xs text-faint">—</span>
              ) : (
                <div className="space-y-0.5">
                  {nodes.map((n) => (
                    <div key={n.server_id} className="flex min-w-0 items-center gap-2 text-xs">
                      <span className="min-w-0 max-w-[180px] truncate">
                        <NodeLink id={n.server_id} />
                      </span>
                      <StateChip value={n.status} />
                    </div>
                  ))}
                </div>
              )}
            </Td>
            <Td className="font-mono text-[11px] whitespace-nowrap tabular-nums">
              r{s.revision}
              <span className="text-faint"> · deployed {s.deployed_revision ? `r${s.deployed_revision}` : "—"}</span>
              {s.deployed_revision > 0 && s.revision > s.deployed_revision && (
                <span className="ml-1.5">
                  <Badge tone="warn">undeployed changes</Badge>
                </span>
              )}
            </Td>
            <Td className="text-xs whitespace-nowrap text-muted">
              <span title={time(s.updated_at)}>{s.updated_at ? ago(s.updated_at) : "—"}</span>
            </Td>
          </tr>
        );
      })}
    </Table>
  );
}
