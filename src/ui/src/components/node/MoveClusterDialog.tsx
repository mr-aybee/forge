import { Link } from "@tanstack/react-router";
import { ArrowRight, Loader2, TriangleAlert } from "lucide-react";
import { useEffect, useState } from "react";

import { ApiError } from "../../api/client";
import { useServices, useAssignServer, useClusters } from "../../api/hooks";
import { type Server, nodeName } from "../../api/types";
import { errorText, useToast } from "../../app/toast";
import { Badge, Button, Field, Modal, Select } from "../ui";

/**
 * Cluster handover: pick the new cluster, see from → to and what happens,
 * then "Move node". The server refuses (409) while the node still holds a
 * role in the old cluster (ingress edge); that
 * message is shown as a blocker, with links to where the role is moved.
 */
export function MoveClusterDialog({ node, open, onOpenChange }: { node: Server; open: boolean; onOpenChange: (o: boolean) => void }) {
  const clusters = useClusters().data ?? [];
  const assign = useAssignServer();
  const toast = useToast();
  const [to, setTo] = useState("");
  const [blocker, setBlocker] = useState<string>();
  const from = node.cluster_id ?? null;
  const fromCluster = clusters.find((c) => c.id === from);
  const toCluster = clusters.find((c) => c.id === to);
  const choices = clusters.filter((c) => c.id !== from);
  // Services in the old cluster that run on this node.
  const apps = useServices(undefined, from, open && !!from);
  const dependents = open && from ? (Array.isArray(apps.data) ? apps.data : []).filter((a) => (a.instances ?? []).some((i) => i?.server_id === node.id)) : [];

  useEffect(() => {
    if (!open) return;
    setTo("");
    setBlocker(undefined);
    assign.reset();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const target = to === "none" ? null : to || undefined;
  const fromName = fromCluster?.name ?? (from ? from.slice(0, 8) : "No cluster");
  const toName = to === "none" ? "No cluster" : (toCluster?.name ?? "—");
  const name = nodeName(node);

  const submit = () => {
    if (target === undefined) return;
    setBlocker(undefined);
    assign.mutate(
      { id: node.id, cluster_id: target, environment_id: node.environment_id },
      {
        onSuccess: () => {
          toast(target ? `${name} is moving to ${toName}; the network handover continues in the background` : `${name} left ${fromName}`);
          onOpenChange(false);
        },
        onError: (e) => {
          if (e instanceof ApiError && e.status === 409) setBlocker(e.message);
          else toast(errorText(e), "error");
        },
      },
    );
  };

  return (
    <Modal open={open} onOpenChange={onOpenChange} title={`Move ${name} to another cluster`} description="The node keeps its agent, packages and data; its cluster membership and network change.">
      <div className="space-y-4 text-[13px]">
        <Field label="Move to">
          <Select value={to} onChange={(e) => setTo(e.target.value)} aria-label="Target cluster">
            <option value="">Choose a cluster…</option>
            {choices.map((c) => (
              <option key={c.id} value={c.id}>
                {c.name}
              </option>
            ))}
            {from && <option value="none">No cluster (leave {fromName})</option>}
          </Select>
        </Field>

        <div className="flex min-w-0 items-center gap-2 rounded-md border border-line px-3 py-2">
          <span className="min-w-0 truncate" title={fromName}>
            {from ? <Badge tone="accent">{fromName}</Badge> : <span className="text-faint">No cluster</span>}
          </span>
          <ArrowRight className="size-3.5 shrink-0 text-faint" />
          <span className="min-w-0 truncate" title={toName}>
            {target ? <Badge tone="accent">{toName}</Badge> : <span className="text-faint">{to === "none" ? "No cluster" : "—"}</span>}
          </span>
        </div>

        <div>
          <div className="mb-1 text-xs font-medium text-muted">What happens</div>
          <ol className="list-decimal space-y-1 pl-5 text-xs text-muted">
            {from && <li>The node leaves {fromName}&apos;s network: Forge removes that cluster&apos;s network interface from it in the background.</li>}
            {target && <li>It joins {toName}&apos;s network and gets an address there; the cluster reconciles within a minute.</li>}
            <li>Who can manage the node follows its cluster. You need rights on both clusters.</li>
            {from && <li>Services in {fromName} that pointed at this node (services, routes, firewall rules) must be moved or updated.</li>}
          </ol>
        </div>

        {from && (
          <div>
            <div className="mb-1 text-xs font-medium text-muted">Depends on this node in {fromName}</div>
            {apps.isLoading ? (
              <p className="text-xs text-faint">Checking services…</p>
            ) : dependents.length ? (
              <ul className="flex flex-wrap gap-1.5">
                {dependents.map((a) => (
                  <li key={a.id}>
                    <Link to="/services/$serviceId" params={{ serviceId: a.id }} className="text-xs hover:text-accent" onClick={() => onOpenChange(false)}>
                      <Badge tone="warn">{a.name}</Badge>
                    </Link>
                  </li>
                ))}
              </ul>
            ) : (
              <p className="text-xs text-faint">{apps.error ? "Could not check services." : "No services run on this node."}</p>
            )}
          </div>
        )}

        {blocker && (
          <div role="alert" className="rounded-md border border-bad/40 bg-bad-soft px-3 py-2 text-bad">
            <div className="flex items-center gap-1.5 text-[13px] font-medium">
              <TriangleAlert className="size-3.5" /> Can&apos;t move yet
            </div>
            <p className="mt-1 text-xs break-words">{blocker}</p>
            {from && (
              <p className="mt-1.5 flex flex-wrap gap-3 text-xs">
                <Link to="/clusters/$clusterId" params={{ clusterId: from }} search={{ tab: "network" }} className="underline" onClick={() => onOpenChange(false)}>
                  {fromName} → Network
                </Link>
                <Link to="/ingress" search={{ cluster: from }} className="underline" onClick={() => onOpenChange(false)}>
                  {fromName} → Ingress
                </Link>
              </p>
            )}
          </div>
        )}

        <p className="text-[11px] text-faint">Moving a node is a sensitive action: you may be asked for an authenticator code.</p>

        <div className="flex justify-end gap-2">
          <Button onClick={() => onOpenChange(false)}>Cancel</Button>
          <Button variant="primary" disabled={target === undefined || assign.isPending} onClick={submit}>
            {assign.isPending && <Loader2 className="size-4 animate-spin" />} Move node
          </Button>
        </div>
      </div>
    </Modal>
  );
}
