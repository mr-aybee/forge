import { Link } from "@tanstack/react-router";
import { Cable } from "lucide-react";

import { useServers, useServicePorts } from "../../api/hooks";
import { nodeName, type ServicePort } from "../../api/types";
import { CopyValue, Skeleton } from "../ui";
import { portCommand } from "./parts";

/**
 * Read-only "Ports" block for a service page: port · role · nodes · the
 * `forge tunnel open …` line that reaches it. Thin border, one line per port.
 * Renders nothing when the service has no known ports.
 */
export function ServicePorts({ kind, service, only, className = "" }: { kind: "app" | "stack" | "package"; service: string | undefined; /** Stack pages for one service: show only this service. */ only?: string; className?: string }) {
  const q = useServicePorts(service, kind);
  const nodes = useServers().data ?? [];
  const data = q.data;
  if (!service) return null;
  if (q.isLoading) return <Skeleton rows={1} />;
  if (!data || !data.supported) return null;

  // One line per (service, port, role); nodes collected.
  const rows = new Map<string, { e: ServicePort; nodes: ServicePort[] }>();
  for (const e of data.entries) {
    if (only && e.name !== only) continue;
    const key = `${e.name}:${e.port}:${e.role ?? ""}`;
    const r = rows.get(key);
    if (r) r.nodes.push(e);
    else rows.set(key, { e, nodes: [e] });
  }
  const perService = new Map<string, number>();
  for (const { e } of rows.values()) perService.set(e.name, (perService.get(e.name) ?? 0) + 1);
  if (rows.size === 0 && !data.error) return null;

  const label = (e: ServicePort) => {
    const n = nodes.find((s) => s.id === e.server_id);
    return n ? nodeName(n) : e.server || e.server_id.slice(0, 8);
  };

  return (
    <section className={`rounded-md border border-line ${className}`} aria-label="Ports">
      <header className="flex items-center gap-1.5 border-b border-line px-3 py-1.5 text-[11px] font-semibold tracking-wide text-faint uppercase">
        <Cable className="size-3.5" aria-hidden /> Ports
        <span className="ml-auto font-normal normal-case tracking-normal">reach them with <span className="font-mono">forge tunnel</span>; the console cannot open one</span>
      </header>
      {data.error ? (
        <p className="px-3 py-2 text-xs text-bad">{data.error}</p>
      ) : (
        <ul className="divide-y divide-line">
          {[...rows.values()].map(({ e, nodes: on }) => {
            const several = (perService.get(e.name) ?? 0) > 1;
            const cmd = portCommand(on[0] ?? e, kind, service, several);
            return (
              <li key={`${e.name}:${e.port}:${e.role ?? ""}`} className="flex flex-wrap items-center gap-x-3 gap-y-1 px-3 py-1.5 text-xs">
                <span className="w-14 shrink-0 font-mono text-[11px] tabular-nums">{e.port}</span>
                <span className="w-16 shrink-0 truncate text-muted" title={e.role ?? undefined}>
                  {e.role ?? "—"}
                </span>
                {kind === "stack" && (
                  <span className="w-24 shrink-0 truncate font-mono text-[11px]" title={e.name}>
                    {e.name}
                  </span>
                )}
                <span className="flex min-w-0 flex-1 flex-wrap items-center gap-x-1.5 gap-y-0.5">
                  {on.map((n, i) => (
                    <Link key={n.server_id} to="/nodes/$nodeId" params={{ nodeId: n.server_id }} className="truncate hover:text-accent" title={n.dial_ip ? `${label(n)} · dials ${n.dial_ip}` : label(n)}>
                      {label(n)}
                      {i < on.length - 1 ? "," : ""}
                    </Link>
                  ))}
                </span>
                <CopyValue text={cmd} label="command" className="max-w-full sm:max-w-[360px]" />
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}
