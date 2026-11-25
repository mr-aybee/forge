import {
  Eraser,
  Globe,
  Pencil,
  Plus,
  RefreshCw,
  Rocket,
  Trash2,
  Undo2,
} from "lucide-react";
import { Link } from "@tanstack/react-router";
import { useState } from "react";

import { useClusters, useServers } from "../../api/hooks";
import {
  type Site,
  ruleKindLabel,
  siteKinds,
  useCertificates,
  useDeleteSite,
  useRenewSite,
  useSiteRun,
  useSites,
} from "../../api/ingress";
import { errorText, useToast } from "../../app/toast";
import {
  Badge,
  Button,
  Confirm,
  Empty,
  ErrorNote,
  Panel,
  RowMenu,
  type RowAction,
  Select,
  Skeleton,
  Table,
  Td,
  ago,
  time,
  useRowMenu,
} from "../ui";
import { EndpointDialog } from "./EndpointDialog";
import { EndpointEditor } from "./EndpointEditor";
import { CertChip, placementLabel } from "./EndpointParts";

type Pending = { site: Site; what: "remove" | "delete" };

/**
 * Every endpoint (Nginx site), whatever serves it: single nodes or a
 * cluster's edge group. `scope` pins the list to one cluster (no filter).
 */
export function Endpoints({
  scope,
  defaultFilter = "all",
}: {
  scope?: string;
  defaultFilter?: string;
}) {
  const toast = useToast();
  const sites = useSites();
  const certs = useCertificates().data ?? [];
  const nodes = useServers().data ?? [];
  const clusters = useClusters().data ?? [];
  const run = useSiteRun();
  const renew = useRenewSite();
  const del = useDeleteSite();
  const rm = useRowMenu();
  const [ui, setUi] = useState<{
    filter: string;
    editing?: string;
    creating: boolean;
    pending?: Pending;
    busy?: string;
  }>({ filter: defaultFilter, creating: false });
  const patch = (p: Partial<typeof ui>) => setUi((u) => ({ ...u, ...p }));
  const host = (id: string) =>
    nodes.find((n) => n.id === id)?.hostname ?? id.slice(0, 8);
  const clusterName = (id: string) =>
    clusters.find((c) => c.id === id)?.name ?? id.slice(0, 8);
  const filter = scope ?? ui.filter;
  const list = (sites.data ?? []).filter((s) => {
    const p = s.config.placement;
    if (filter === "all") return true;
    if (filter === "nodes") return p.kind === "nodes";
    return p.kind === "cluster" && p.cluster_id === filter;
  });

  const edgeRun = (s: Site, action: "apply" | "remove" | "cache/clear") => {
    patch({ busy: s.id });
    run.mutate(
      { id: s.id, action },
      {
        onSuccess: (r) => {
          const failed = (r.edges ?? []).filter(
            (e) => e.result?.status !== "succeeded",
          );
          const verb =
            action === "apply"
              ? "Apply"
              : action === "remove"
                ? "Remove"
                : "Clearing the cache";
          if (failed.length)
            toast(
              `${verb} failed on ${failed.map((x) => host(x.server_id)).join(", ")}: ${failed[0]?.result?.error ?? ""}`,
              "error",
            );
          else {
            const n = r.edges?.length ?? 0;
            const dns = (r.dns ?? []).filter(
              (d) => d.status === "succeeded",
            ).length;
            toast(
              `${action === "apply" ? "Applied" : action === "remove" ? "Removed" : "Cache cleared"} on ${n} edge${n === 1 ? "" : "s"}${dns ? ` · ${dns} DNS records published` : ""}`,
            );
          }
        },
        onError: (e) => toast(errorText(e), "error"),
        onSettled: () => patch({ busy: undefined }),
      },
    );
  };

  return (
    <>
      <Panel
        title={`${list.length} endpoint${list.length === 1 ? "" : "s"}`}
        description="Domains served by Nginx on single nodes or a cluster's edge group. Saving never applies; Apply pushes the config to the edges."
        actions={
          <>
            {!scope && (
              <div className="w-48">
                <Select
                  aria-label="Filter endpoints by placement"
                  value={ui.filter}
                  onChange={(e) => patch({ filter: e.target.value })}
                >
                  <option value="all">All endpoints</option>
                  <option value="nodes">Single nodes</option>
                  {clusters.map((c) => (
                    <option key={c.id} value={c.id}>
                      Cluster {c.name}
                    </option>
                  ))}
                </Select>
              </div>
            )}
            <Button variant="primary" onClick={() => patch({ creating: true })}>
              <Plus className="size-4" /> New endpoint
            </Button>
          </>
        }
      >
        {sites.error ? (
          <ErrorNote error={sites.error} />
        ) : sites.isLoading ? (
          <Skeleton />
        ) : !list.length ? (
          <Empty
            icon={<Globe className="size-5" />}
            title={
              sites.data?.length ? "No endpoints here" : "No endpoints yet"
            }
            action={
              <Button onClick={() => patch({ creating: true })}>
                New endpoint
              </Button>
            }
          >
            {sites.data?.length ? (
              "No endpoint matches this filter."
            ) : (
              <>
                Route a domain to a service, a node port or any address. You
                need Nginx on a node (
                <Link
                  to="/app-store/$packageId"
                  params={{ packageId: "nginx" }}
                  className="text-accent hover:underline"
                >
                  App Store
                </Link>
                ) or a cluster with an ingress profile.
              </>
            )}
          </Empty>
        ) : (
          <Table head={["Domain", "Rules", "Served by", "SSL", "Edges", ""]}>
            {list.map((s) => {
              const spec = s.config.spec;
              const busy = ui.busy === s.id;
              const why = busy
                ? "An action on this endpoint is running"
                : undefined;
              const applied = Object.values(s.edge_status).some(
                (e) => e.status === "applied",
              );
              const actions: RowAction[] = [
                {
                  label: "Edit",
                  icon: Pencil,
                  onSelect: () => patch({ editing: s.id }),
                },
                {
                  label: busy ? "Working…" : "Apply to edges",
                  icon: Rocket,
                  tone: "ok",
                  disabled: busy || !s.config.edges.length,
                  hint:
                    why ??
                    (s.config.edges.length
                      ? undefined
                      : "No edges: choose where it's served (Edit → Placement)"),
                  onSelect: () => edgeRun(s, "apply"),
                },
                {
                  label: "Renew certificate now",
                  icon: RefreshCw,
                  hidden: spec.tls !== "acme",
                  disabled: busy || renew.isPending,
                  hint: why,
                  onSelect: () =>
                    renew.mutate(s.id, {
                      onSuccess: (r) => {
                        const bad = Object.entries(r.certificates ?? {}).find(
                          ([, c]) => c?.renew_error,
                        );
                        toast(
                          bad
                            ? `Renewal failed on ${host(bad[0])}: ${bad[1]?.renew_error}`
                            : "Certificate renewed on every edge",
                          bad ? "error" : "ok",
                        );
                      },
                      onError: (e) => toast(errorText(e), "error"),
                    }),
                },
                {
                  label: "Clear cache",
                  icon: Eraser,
                  hidden: !spec.cache,
                  disabled: busy,
                  hint: why,
                  onSelect: () => edgeRun(s, "cache/clear"),
                },
                {
                  label: "Remove from edges",
                  icon: Undo2,
                  danger: true,
                  disabled: busy || !applied,
                  hint: why ?? (applied ? undefined : "Not applied anywhere"),
                  onSelect: () =>
                    patch({ pending: { site: s, what: "remove" } }),
                },
                {
                  label: "Delete",
                  icon: Trash2,
                  danger: true,
                  disabled: busy || applied,
                  hint: applied ? "Remove it from its edges first" : why,
                  onSelect: () =>
                    patch({ pending: { site: s, what: "delete" } }),
                },
              ];
              const primary = spec.domains[0];
              const scheme = spec.tls === "off" ? "http" : "https";
              const last = Math.max(
                0,
                ...Object.values(s.edge_status).map((e) => e.updated_at ?? 0),
              );
              return (
                <tr
                  key={s.id}
                  className="hover:bg-panel-2/60"
                  {...rm.rowProps(actions, `Actions for ${s.name}`)}
                >
                  <Td className="max-w-[280px]">
                    <div className="flex min-w-0 items-center gap-1">
                      {primary ? (
                        <a
                          href={`${scheme}://${primary.replace(/^\*\./, "")}`}
                          target="_blank"
                          rel="noreferrer"
                          className="truncate font-medium hover:text-accent"
                          title={`${scheme}://${primary}`}
                        >
                          {primary}
                        </a>
                      ) : (
                        <span className="text-faint">—</span>
                      )}
                      {spec.domains.length > 1 && (
                        <span
                          className="shrink-0 text-xs text-faint"
                          title={spec.domains.slice(1).join("\n")}
                        >
                          +{spec.domains.length - 1}
                        </span>
                      )}
                    </div>
                    <button
                      type="button"
                      className="block max-w-full truncate font-mono text-[11px] text-faint hover:text-accent"
                      onClick={() => patch({ editing: s.id })}
                      title={`Edit ${s.name}`}
                    >
                      {s.name}
                    </button>
                  </Td>
                  <Td>
                    <div className="flex flex-wrap gap-1">
                      {siteKinds(spec).map((k) => (
                        <Badge
                          key={k}
                          tone={k === "balancer" ? "accent" : "muted"}
                        >
                          {ruleKindLabel(k)}
                        </Badge>
                      ))}
                      {spec.custom.trim() && <Badge>custom</Badge>}
                    </div>
                  </Td>
                  <Td className="max-w-[220px] text-xs">
                    <span
                      className="block truncate"
                      title={placementLabel(s, host, clusterName)}
                    >
                      {placementLabel(s, host, clusterName)}
                    </span>
                  </Td>
                  <Td>
                    <CertChip site={s} certs={certs} />
                  </Td>
                  <Td>
                    <div className="flex flex-wrap items-center gap-1.5">
                      {s.config.edges.length === 0 && (
                        <span className="text-xs text-faint">—</span>
                      )}
                      {s.config.edges.map((e) => {
                        const st = s.edge_status[e];
                        const tone =
                          st?.status === "applied"
                            ? "bg-ok"
                            : st?.status === "failed"
                              ? "bg-bad"
                              : "bg-faint";
                        return (
                          <span
                            key={e}
                            role="img"
                            aria-label={`${host(e)}: ${st?.status ?? "not applied"}`}
                            title={`${host(e)}: ${st?.status ?? "not applied"}${st?.error ? ` · ${st.error}` : ""}`}
                            className={`size-2 rounded-full ${tone}`}
                          />
                        );
                      })}
                      {last > 0 && (
                        <span
                          className="text-[11px] text-faint"
                          title={time(last)}
                        >
                          {ago(last)}
                        </span>
                      )}
                    </div>
                  </Td>
                  <Td className="text-right">
                    <RowMenu
                      actions={actions}
                      label={`Actions for ${s.name}`}
                    />
                  </Td>
                </tr>
              );
            })}
          </Table>
        )}
      </Panel>
      {rm.menu}
      {ui.creating && (
        <EndpointDialog
          clusterId={scope}
          onClose={() => patch({ creating: false })}
          onCreated={(s) => patch({ creating: false, editing: s.id })}
        />
      )}
      {ui.editing && (
        <EndpointEditor
          siteId={ui.editing}
          onClose={() => patch({ editing: undefined })}
        />
      )}
      {ui.pending && (
        <Confirm
          open
          onOpenChange={(o) => !o && patch({ pending: undefined })}
          title={
            ui.pending.what === "remove"
              ? `Take ${ui.pending.site.name} off its edges?`
              : `Delete ${ui.pending.site.name}?`
          }
          body={
            ui.pending.what === "remove" ? (
              <>
                Removes the Nginx config (after{" "}
                <span className="font-mono">nginx -t</span>) on{" "}
                {ui.pending.site.config.edges.map(host).join(", ") ||
                  "its edges"}
                . {ui.pending.site.config.spec.domains.join(", ")} stop
                answering there. DNS records are left as they are.
              </>
            ) : (
              "Deletes the saved endpoint. It is not on any edge, so no traffic changes."
            )
          }
          confirmLabel={
            ui.pending.what === "remove" ? "Remove from edges" : "Delete"
          }
          onConfirm={() => {
            const p = ui.pending;
            patch({ pending: undefined });
            if (!p) return;
            if (p.what === "remove") edgeRun(p.site, "remove");
            else
              del.mutate(p.site.id, {
                onSuccess: () => toast(`Endpoint ${p.site.name} deleted`),
                onError: (e) => toast(errorText(e), "error"),
              });
          }}
        />
      )}
    </>
  );
}
