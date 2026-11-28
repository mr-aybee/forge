import { Copy, Loader2, ShieldAlert, TriangleAlert } from "lucide-react";
import { useState } from "react";

import { useContainerQuery } from "../api/hooks";
import type { Service, Server } from "../api/types";
import { Button, Modal } from "./ui";

export type PlacementRole = "active" | "standby";

const ROLES: { id: PlacementRole; title: string; body: string; icon: typeof Copy }[] = [
  { id: "active", title: "Replica", body: "Active. Serves traffic alongside the other active nodes; an ingress endpoint load-balances across replicas.", icon: Copy },
  { id: "standby", title: "Standby", body: "Takes over only when every replica is down (Nginx backup).", icon: ShieldAlert },
];

/**
 * "Run <service> on <node>": pick the node's role before a second (or later)
 * placement. Container services need Docker on the node; the dialog checks
 * with the node's `info` op and blocks Confirm while it is missing.
 */
export function PlacementDialog({ app, node, busy, onConfirm, onClose }: { app: Service; node: Server; busy?: boolean; onConfirm: (role: PlacementRole) => void; onClose: () => void }) {
  const [role, setRole] = useState<PlacementRole>("active");
  const needsDocker = app.runtime === "container";
  const online = node.status === "online";
  const info = useContainerQuery<unknown>(node.id, "info", needsDocker && online);
  // `info` succeeds only with Docker Engine present; a failed op means it is missing.
  const noDocker = needsDocker && online && !!info.data && info.data.status !== "succeeded" && !info.isFetching;
  const unchecked = needsDocker && (!online || info.isError);
  const replicas = app.instances.filter((i) => i.role !== "standby").length;
  return (
    <Modal open onOpenChange={(o) => !o && onClose()} title={`Run ${app.name} on ${node.hostname}`} description={`${app.name} already runs on ${app.instances.length} node${app.instances.length === 1 ? "" : "s"} (${replicas} replica${replicas === 1 ? "" : "s"}). Choose this node's role.`}>
      <div role="radiogroup" aria-label="Role" className="space-y-2">
        {ROLES.map((r) => {
          const on = role === r.id;
          return (
            <label key={r.id} className={`flex cursor-pointer items-start gap-3 rounded-md border px-3 py-2 transition ${on ? "border-accent bg-accent-soft/40" : "border-line hover:border-line-strong"}`}>
              <input type="radio" name="placement-role" className="mt-1" checked={on} onChange={() => setRole(r.id)} />
              <span className="min-w-0">
                <span className="flex items-center gap-1.5 text-[13px] font-medium">
                  <r.icon className="size-3.5 text-accent" aria-hidden /> {r.title}
                </span>
                <span className="block text-xs text-muted">{r.body}</span>
              </span>
            </label>
          );
        })}
      </div>
      {needsDocker && (
        <p className={`mt-3 flex items-start gap-1.5 text-xs ${noDocker ? "text-bad" : "text-muted"}`}>
          {noDocker ? (
            <>
              <TriangleAlert className="mt-px size-3.5 shrink-0" aria-hidden /> Docker Engine is not installed on this node. Install it from the App Store first.
            </>
          ) : unchecked ? (
            <>
              <TriangleAlert className="mt-px size-3.5 shrink-0" aria-hidden /> Could not check Docker on this node ({online ? "the check failed" : "node offline"}); the deploy fails if Docker Engine is missing.
            </>
          ) : info.isFetching || !info.data ? (
            <>
              <Loader2 className="mt-px size-3.5 shrink-0 animate-spin" aria-hidden /> Checking Docker Engine on {node.hostname}…
            </>
          ) : (
            <>Docker Engine is installed on {node.hostname}.</>
          )}
        </p>
      )}
      <p className="mt-3 text-xs text-faint">Nothing runs until you press Deploy. An endpoint already in front of this service picks the node up when you apply it again.</p>
      <div className="mt-4 flex justify-end gap-2">
        <Button variant="ghost" onClick={onClose}>
          Cancel
        </Button>
        <Button variant="primary" disabled={busy || noDocker || (needsDocker && online && !info.data && !info.isError)} title={noDocker ? "Docker Engine is not installed on this node" : undefined} onClick={() => onConfirm(role)}>
          {busy && <Loader2 className="size-4 animate-spin" />} Confirm
        </Button>
      </div>
    </Modal>
  );
}
