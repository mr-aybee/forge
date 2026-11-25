// RustFS containers found on the workspace's nodes: run by a store, left behind by Forge, or created elsewhere.
import { Link } from "@tanstack/react-router";
import { Copy, PackageCheck, RefreshCw } from "lucide-react";
import { useState } from "react";

import { ApiError } from "../../api/client";
import { useDiscoverStores } from "../../api/storage";
import type { DiscoveredContainer, DiscoverNode } from "../../api/types";
import { useToast } from "../../app/toast";
import { Badge, Button, ErrorNote, IconButton, Panel, Skeleton, Status, Table, Td } from "../ui";
import { AdoptStoreDialog } from "./StoreDialogs";

const KIND: Record<string, { label: string; tone: string; hint: string }> = {
  managed: { label: "managed", tone: "ok", hint: "A Forge store runs this container." },
  orphaned: { label: "orphaned", tone: "warn", hint: "Created by Forge, but no store owns it any more. Adopt it to manage it again." },
  foreign: { label: "foreign", tone: "muted", hint: "Created outside Forge. Adopt it to manage it as a store." },
};

const DOCKER: Record<string, string> = {
  not_installed: "Docker not installed",
  unreachable: "Docker unreachable",
  unsupported_os: "Docker not supported on this OS",
};

/** Docker state of every node that could not be scanned. */
function NodeLines({ nodes }: { nodes: DiscoverNode[] }) {
  if (!nodes.length) return null;
  return (
    <ul className="divide-y divide-line border-t border-line text-xs">
      {nodes.map((n) => (
        <li key={n.server_id} className="flex min-w-0 items-center gap-2 px-4 py-1.5">
          <Link to="/nodes/$nodeId" params={{ nodeId: n.server_id }} className="shrink-0 font-medium hover:text-accent">
            {n.hostname}
          </Link>
          <span className={n.docker === "unreachable" ? "text-warn" : "text-muted"}>{DOCKER[n.docker] ?? n.docker}</span>
          {n.error && (
            <span className="min-w-0 truncate font-mono text-[11px] text-faint" title={n.error}>
              {n.error}
            </span>
          )}
        </li>
      ))}
    </ul>
  );
}

/** "Found on nodes": hidden on servers without discovery and when nothing was found. */
export function FoundOnNodes() {
  const d = useDiscoverStores();
  const [adopting, setAdopting] = useState<DiscoveredContainer | null>(null);
  const toast = useToast();
  if (d.error instanceof ApiError && (d.error.status === 404 || d.error.status === 403)) return null;
  const containers = d.data?.containers ?? [];
  const problems = (d.data?.nodes ?? []).filter((n) => n.docker !== "available");
  if (d.isSuccess && !containers.length && !problems.length) return null;
  const unmanaged = containers.filter((c) => c.kind !== "managed").length;

  return (
    <Panel
      className="mt-4"
      title="Found on nodes"
      description={`RustFS containers on your Linux nodes${unmanaged ? `; ${unmanaged} not run by a store` : ""}.`}
      actions={
        <IconButton label="Scan again" onClick={() => void d.refetch()}>
          <RefreshCw className={`size-3.5 ${d.isFetching ? "animate-spin" : ""}`} />
        </IconButton>
      }
    >
      {d.isLoading ? (
        <Skeleton rows={2} />
      ) : d.error ? (
        <ErrorNote error={d.error} />
      ) : (
        <>
          {containers.length > 0 ? (
            <Table head={["Node", "Container", "Image", "Ports", "Mounts", "State", "Kind", ""]}>
              {containers.map((c) => {
                const k = KIND[c.kind] ?? KIND.foreign;
                return (
                  <tr key={`${c.server_id}/${c.id || c.name}`} className="text-[13px] hover:bg-panel-2/50">
                    <Td className="max-w-[10rem]">
                      <Link to="/nodes/$nodeId" params={{ nodeId: c.server_id }} className="block truncate hover:text-accent" title={c.hostname}>
                        {c.hostname}
                      </Link>
                    </Td>
                    <Td className="max-w-[14rem]">
                      <span className="flex min-w-0 items-center gap-1">
                        <span className="truncate font-mono text-[11px] select-all" title={`${c.name} (${c.id})`}>
                          {c.name || "—"}
                        </span>
                        {c.name && (
                          <IconButton label={`Copy container name ${c.name}`} className="!size-6 shrink-0" onClick={() => void navigator.clipboard?.writeText(c.name).then(() => toast("Container name copied"))}>
                            <Copy className="size-3" />
                          </IconButton>
                        )}
                      </span>
                      {c.forge_label && <div className="truncate font-mono text-[11px] text-faint">label {c.forge_label}</div>}
                    </Td>
                    <Td className="max-w-[14rem]">
                      <span className="block truncate font-mono text-[11px]" title={c.image}>
                        {c.image || "—"}
                      </span>
                      {c.version && <div className="font-mono text-[11px] text-faint tabular-nums">{c.version}</div>}
                    </Td>
                    <Td className="max-w-[12rem] font-mono text-[11px] tabular-nums">
                      {c.ports.length ? (
                        c.ports.map((p) => (
                          <div key={p} className="truncate" title={p}>
                            {p}
                          </div>
                        ))
                      ) : (
                        <span className="text-faint">{c.network === "host" ? "host network" : "—"}</span>
                      )}
                    </Td>
                    <Td className="max-w-[16rem] font-mono text-[11px]">
                      {c.mounts.length ? (
                        c.mounts.map((m, i) => {
                          const src = m.type === "volume" ? (m.name ?? m.source ?? "?") : (m.source ?? "?");
                          const text = `${src} → ${m.destination ?? "?"}`;
                          return (
                            <div key={i} className="truncate" title={`${m.type}: ${text}`}>
                              {m.type === "volume" && <span className="text-faint">vol </span>}
                              {text}
                            </div>
                          );
                        })
                      ) : (
                        <span className="text-faint">—</span>
                      )}
                    </Td>
                    <Td title={c.status ?? undefined}>
                      <Status value={c.state || "unknown"} />
                    </Td>
                    <Td>
                      <span title={k?.hint}>
                        <Badge tone={k?.tone}>{k?.label ?? c.kind}</Badge>
                      </span>
                    </Td>
                    <Td className="text-right whitespace-nowrap">
                      {c.kind === "managed" ? (
                        c.store_id ? (
                          <Link to="/storage/$storeId" params={{ storeId: c.store_id }} className="text-xs text-accent hover:underline">
                            Open store →
                          </Link>
                        ) : (
                          <span className="text-xs text-faint">—</span>
                        )
                      ) : (
                        <Button onClick={() => setAdopting(c)} title={k?.hint}>
                          <PackageCheck className="size-4" /> Adopt
                        </Button>
                      )}
                    </Td>
                  </tr>
                );
              })}
            </Table>
          ) : (
            <p className="px-4 py-3 text-xs text-muted">No RustFS containers found on the nodes that could be scanned.</p>
          )}
          <NodeLines nodes={problems} />
        </>
      )}
      <AdoptStoreDialog open={!!adopting} onOpenChange={(o) => !o && setAdopting(null)} container={adopting} />
    </Panel>
  );
}
