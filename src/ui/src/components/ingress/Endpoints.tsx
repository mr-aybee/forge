import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Globe, Link2, Pencil, Plus, RefreshCw, Rocket, Undo2 } from "lucide-react";
import { useState } from "react";

import { api, get, post } from "../../api/client";
import { useServers } from "../../api/hooks";
import { errorText, useToast } from "../../app/toast";
import { Badge, Button, Confirm, Empty, Panel, RowMenu, type RowAction, Skeleton, Status, Table, Td, ago, useRowMenu } from "../ui";
import { EndpointDialog } from "./EndpointDialog";
import { useSites } from "./sites";
import { targetLabel } from "./TargetEditor";
import { type Certificate, type EdgeCert, type Member, type Site } from "./types";


/** Certificate chip: green while valid (Forge renews Let's Encrypt at 5 days left). */
function CertChip({ site, edges, certs }: { site: Site; edges: string[]; certs: Certificate[] }) {
  const tls = site.config.spec.tls;
  if (tls === "off") return <Badge>HTTP</Badge>;
  if (tls === "uploaded") {
    const c = certs.find((x) => x.id === site.config.spec.cert_id);
    if (!c) return <Badge tone="bad">certificate missing</Badge>;
    const tone = c.days_left < 0 ? "bad" : c.days_left <= 14 ? "warn" : "ok";
    return (
      <span title={c.days_left <= 14 ? "Uploaded certificates cannot renew themselves: upload the new one from your CA and switch this endpoint to it." : undefined}>
        <Badge tone={tone}>{c.days_left < 0 ? "certificate expired" : `${c.name} · ${c.days_left}d`}</Badge>
      </span>
    );
  }
  const states = edges.map((e) => site.edge_status[e]?.cert).filter((c): c is EdgeCert => !!c);
  const failing = states.find((c) => c.renew_error);
  const days = states.map((c) => c.days_left).filter((d): d is number => typeof d === "number");
  const min = days.length ? Math.min(...days) : undefined;
  if (failing) {
    return (
      <span title={failing.renew_error ?? undefined}>
        <Badge tone="bad">renewal failing{min !== undefined ? ` · ${min}d` : ""}</Badge>
      </span>
    );
  }
  return (
    <span title="Let's Encrypt: Forge checks every 6 hours and renews automatically when 5 days or fewer are left.">
      <Badge tone="ok">Let's Encrypt{min !== undefined ? ` · ${min}d · auto-renews` : " · auto-renews"}</Badge>
    </span>
  );
}


/**
 * Service endpoints (Nginx sites). With `clusterId`, only that cluster's
 * endpoints are listed and new ones are served by its ingress profile.
 */
export function Endpoints({ clusterId, members = [], hasProfile = false }: { clusterId?: string; members?: Member[]; hasProfile?: boolean }) {
  const qc = useQueryClient();
  const toast = useToast();
  const all = useSites();
  // A cluster lists its linked endpoints and the unlinked ones whose explicit
  // edges are its nodes (they stay on the workspace page too).
  const memberIds = new Set(members.map((m) => m.id));
  const unlinkedHere = (s: Site) => !!clusterId && !s.config.cluster_id && (s.config.edges ?? []).some((e) => memberIds.has(e));
  const sites = {
    isLoading: all.isLoading,
    data: all.data?.filter((s) => (clusterId ? s.config.cluster_id === clusterId || unlinkedHere(s) : !s.config.cluster_id)),
  };
  const nodes = useServers().data ?? [];
  const certs = useQuery({ queryKey: ["certificates"], queryFn: () => get<Certificate[]>("/api/v1/certificates") }).data ?? [];
  const renew = useMutation({
    mutationFn: (id: string) => post<{ ok: boolean; certificates: Record<string, EdgeCert | null> }>(`/api/v1/sites/${id}/renew`),
    onMutate: (id) => setBusy(id),
    onSettled: () => {
      setBusy(undefined);
      void qc.invalidateQueries({ queryKey: ["sites"] });
    },
    onSuccess: (r) => {
      const bad = Object.entries(r.certificates).find(([, c]) => c?.renew_error);
      if (bad) toast(`Renewal failed on ${host(bad[0])}: ${bad[1]?.renew_error}`, "error");
      else toast("Certificate renewed on every edge");
    },
    onError: (e) => toast(errorText(e), "error"),
  });
  const [editing, setEditing] = useState<Site | "new">();
  const [busy, setBusy] = useState<string>();
  const [removing, setRemoving] = useState<Site>();
  const [linking, setLinking] = useState<Site>();
  const rm = useRowMenu();
  const host = (id: string) => nodes.find((n) => n.id === id)?.hostname ?? id.slice(0, 8);

  const act = useMutation({
    mutationFn: ({ id, action }: { id: string; action: "apply" | "remove" }) => post<{ edges: { server_id: string; result: { status: string; error?: string } }[]; dns?: { name: string; type: string; status: string; error?: string }[] | null }>(`/api/v1/sites/${id}/${action}`),
    onMutate: ({ id }) => setBusy(id),
    onSettled: () => {
      setBusy(undefined);
      void qc.invalidateQueries({ queryKey: ["sites"] });
    },
    onSuccess: (r, { action }) => {
      const failed = r.edges.filter((e) => e.result.status !== "succeeded");
      if (failed.length) toast(`${action} failed on ${failed.map((f) => host(f.server_id)).join(", ")}: ${failed[0]!.result.error ?? ""}`, "error");
      else toast(`${action === "apply" ? "Applied" : "Removed"} on ${r.edges.length} edge node${r.edges.length === 1 ? "" : "s"}${r.dns?.length ? ` · ${r.dns.filter((d) => d.status === "succeeded").length} DNS records published` : ""}`);
    },
    onError: (e) => toast(errorText(e), "error"),
  });
  const link = useMutation({
    mutationFn: (s: Site) =>
      api<Site>("PUT", `/api/v1/sites/${s.id}`, {
        ...s.config.spec,
        name: s.name,
        edges: [],
        cluster_id: clusterId,
        targets: s.config.targets ?? [],
        standby_targets: s.config.standby_targets ?? [],
        dns: s.config.dns ?? null,
      }),
    onSuccess: (s) => {
      toast(`${s.name} now follows the cluster's ingress profile; press Apply to push it to the edges`);
      void qc.invalidateQueries({ queryKey: ["sites"] });
      void qc.invalidateQueries({ queryKey: ["cluster-routing", clusterId] });
    },
    onError: (e) => toast(errorText(e), "error"),
  });
  const del = useMutation({
    mutationFn: (id: string) => api<void>("DELETE", `/api/v1/sites/${id}`),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ["sites"] }),
    onError: (e) => toast(errorText(e), "error"),
  });

  return (
    <>
      <Panel
        title={`${sites.data?.length ?? 0} endpoint${sites.data?.length === 1 ? "" : "s"}`}
        description={clusterId ? "Domains served by this cluster's edge nodes. Apply pushes the config to every edge." : "Endpoints with hand-picked edge nodes (not tied to a cluster profile)."}
        actions={
          <Button variant="primary" disabled={!!clusterId && !hasProfile} title={clusterId && !hasProfile ? "Configure the ingress profile first" : undefined} onClick={() => setEditing("new")}>
            <Plus className="size-4" /> New endpoint
          </Button>
        }
      >
        {sites.isLoading ? (
          <Skeleton />
        ) : !sites.data?.length ? (
          <Empty icon={<Globe className="size-5" />} title="No endpoints yet">
            {clusterId
              ? hasProfile
                ? "Register a domain and point it at a service in this cluster, a node port or any address."
                : "Configure the cluster's ingress profile (Profile tab), then register endpoints here."
              : "Install Nginx from the App Store on one or more edge nodes, then route a domain to a service."}
          </Empty>
        ) : (
          <Table head={["Endpoint", "Targets", "Edges", "TLS / DNS", ""]}>
            {sites.data.map((s) => {
              const working = busy === s.id ? "An action on this endpoint is running" : undefined;
              const actions: RowAction[] = [
                { label: busy === s.id ? "Applying…" : "Apply to edges", icon: Rocket, disabled: busy === s.id, hint: working, onSelect: () => act.mutate({ id: s.id, action: "apply" }) },
                { label: "Edit", icon: Pencil, onSelect: () => setEditing(s) },
                ...(unlinkedHere(s)
                  ? [{ label: "Link to cluster", icon: Link2, disabled: !hasProfile || link.isPending, hint: hasProfile ? undefined : "Configure this cluster's ingress profile first", onSelect: () => setLinking(s) }]
                  : []),
                ...(s.config.spec.tls === "acme"
                  ? [{ label: busy === s.id ? "Renewing…" : "Renew certificate now", icon: RefreshCw, disabled: busy === s.id, hint: working, onSelect: () => renew.mutate(s.id) }]
                  : []),
                { label: "Remove from edges", icon: Undo2, danger: true, disabled: busy === s.id, hint: working, onSelect: () => setRemoving(s) },
              ];
              return (
              <tr key={s.id} className="hover:bg-panel-2/60" {...rm.rowProps(actions, `Actions for ${s.name}`)}>
                <Td>
                  <button className="text-left font-medium hover:text-accent" onClick={() => setEditing(s)}>
                    {s.config.spec.domains[0]}
                  </button>
                  {s.config.spec.domains.length > 1 && <span className="ml-1 text-xs text-faint">+{s.config.spec.domains.length - 1}</span>}
                  <div className="text-xs text-faint">
                    {s.name} · updated {ago(s.updated_at)}
                  </div>
                  {unlinkedHere(s) && (
                    <span title="Served by hand-picked edge nodes of this cluster, not by its ingress profile. Use Link to cluster in the row menu.">
                      <Badge tone="warn">explicit edges · not linked to profile</Badge>
                    </span>
                  )}
                </Td>
                <Td className="font-mono text-xs text-muted">
                  {s.config.targets?.length ? s.config.targets.map((t) => targetLabel(t, members)).join(", ") : s.config.spec.upstreams.join(", ")}
                  {s.config.spec.standby.length > 0 && <div className="text-faint">standby {s.config.spec.standby.join(", ")}</div>}
                </Td>
                <Td>
                  <div className="flex flex-wrap gap-1">
                    {s.config.edges.map((e) => {
                      const st = s.edge_status[e];
                      return (
                        <span key={e} title={st?.error ?? undefined} className="inline-flex items-center gap-1 text-xs">
                          <Status value={st?.status === "applied" ? "online" : st?.status === "failed" ? "failed" : "pending"} />
                          {host(e)}
                        </span>
                      );
                    })}
                  </div>
                </Td>
                <Td>
                  <div className="flex flex-wrap gap-1">
                    <CertChip site={s} edges={s.config.edges} certs={certs} />
                    {s.config.dns && <Badge tone="accent">DNS {s.config.dns.mode === "active_standby" ? "active/standby" : "round-robin"}</Badge>}
                    {s.config.spec.custom.trim() && <Badge>custom block</Badge>}
                  </div>
                </Td>
                <Td className="text-right">
                  <RowMenu actions={actions} label={`Actions for ${s.name}`} />
                </Td>
              </tr>
              );
            })}
          </Table>
        )}
      </Panel>
      {rm.menu}
      {editing && (
        <EndpointDialog
          site={editing === "new" ? undefined : editing}
          // Editing an unlinked endpoint from a cluster page keeps its own edges.
          clusterId={editing !== "new" && !editing.config.cluster_id ? undefined : clusterId} members={members} onClose={() => setEditing(undefined)} onDelete={(id) => del.mutate(id)} />
      )}
      <Confirm
        open={!!removing}
        onOpenChange={(o) => !o && setRemoving(undefined)}
        title={`Take ${removing?.name} off its edge nodes?`}
        body="Removes the Nginx config (after nginx -t) on every edge. DNS records are left as they are."
        confirmLabel="Remove from edges"
        onConfirm={() => {
          if (removing) act.mutate({ id: removing.id, action: "remove" });
          setRemoving(undefined);
        }}
      />
      <Confirm
        open={!!linking}
        onOpenChange={(o) => !o && setLinking(undefined)}
        title={`Link ${linking?.name} to this cluster?`}
        body="Its edge nodes then come from the cluster's ingress profile instead of the hand-picked list. Nothing changes on the edges until you press Apply. If a current edge is not in the profile, remove the endpoint from its edges first, or its old config stays on that node."
        confirmLabel="Link to cluster"
        onConfirm={() => {
          if (linking) link.mutate(linking);
          setLinking(undefined);
        }}
      />
    </>
  );
}

