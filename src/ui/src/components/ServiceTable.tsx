import { Link } from "@tanstack/react-router";

import type { Service, Environment } from "../api/types";
import { RUNTIMES } from "./service-editor/sections";
import { StackBadge, isStackService } from "./stacks/StackBadge";
import { usePinAction } from "./Pins";
import { RUNNING_STATES } from "./InstanceState";
import { Badge, RowMenu, Status, Table, Td, ago, time } from "./ui";

/**
 * Services as a dense table: name → `/services/$serviceId`,
 * runtime, nodes, updated. With `environments`, an **Environment** column
 * links each row's environment; the same rows serve the Services page, the
 * Services page and a node.s or cluster.s views.
 */
export function ServiceTable({ apps, environments }: { apps: Service[]; environments?: Environment[] }) {
  const rows = Array.isArray(apps) ? apps : [];
  const envOf = (id: string) => (environments ?? []).find((n) => n.id === id);
  const pinAction = usePinAction();
  return (
    <Table head={environments ? ["Service", "Environment", "Runtime", "Nodes", "Updated", ""] : ["Service", "Runtime", "Nodes", "Updated", ""]}>
      {rows.map((a) => {
        const rt = RUNTIMES.find((r) => r.id === a.runtime);
        const inst = Array.isArray(a.instances) ? a.instances : [];
        const up = inst.filter((i) => !!i.state && RUNNING_STATES.has(i.state)).length;
        const env = environments ? envOf(a.environment_id) : undefined;
        const detail = a.runtime === "container" || isStackService(a) ? a.spec?.image : a.spec?.command;
        return (
          <tr key={a.id} className="hover:bg-panel-2/60">
            <Td className="max-w-[360px] py-1.5">
              <Link to="/services/$serviceId" params={{ serviceId: a.id }} className="block truncate text-[13px] font-medium hover:text-accent" title={a.name}>
                {a.name}
              </Link>
              {detail && (
                <div className="truncate font-mono text-[11px] text-faint select-all" title={detail}>
                  {detail}
                </div>
              )}
            </Td>
            {environments && (
              <Td className="py-1.5">
                {env?.cluster_id ? (
                  <Link to="/projects" search={{ sel: `env:${env.id}` }} className="hover:opacity-80" title={`Environment ${env.name}`}>
                    <Badge tone="accent">{env.name}</Badge>
                  </Link>
                ) : env ? (
                  <Badge tone="accent">{env.name}</Badge>
                ) : (
                  <span className="text-faint">—</span>
                )}
              </Td>
            )}
            <Td className="py-1.5">
              <span className="inline-flex items-center gap-1.5 text-xs text-muted">
                {isStackService(a) ? (
                  <StackBadge a={a} />
                ) : (
                  <>
                    {rt && <rt.icon className="size-3.5 text-accent" />} {rt?.label ?? a.runtime}
                  </>
                )}
              </span>
            </Td>
            <Td className="py-1.5">
              {inst.length === 0 ? (
                <span className="text-xs text-faint">not placed</span>
              ) : (
                <span className="inline-flex items-center gap-2">
                  <Status value={up === inst.length ? "online" : up > 0 ? "running" : "offline"} />
                  <span className="font-mono text-[11px] text-muted tabular-nums">
                    {up}/{inst.length} active
                  </span>
                </span>
              )}
            </Td>
            <Td className="py-1.5 text-xs whitespace-nowrap text-muted">
              <span title={time(a.updated_at)}>{a.updated_at ? ago(a.updated_at) : "—"}</span>
            </Td>
            <Td className="w-8 py-0.5 text-right">
              <RowMenu actions={[pinAction({ kind: "service", id: a.id, label: a.name, sublabel: env?.name })]} label={`Actions for ${a.name}`} />
            </Td>
          </tr>
        );
      })}
    </Table>
  );
}
