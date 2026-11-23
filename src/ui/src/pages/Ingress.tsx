import { useQuery } from "@tanstack/react-query";
import { Activity, FileKey2, Globe, Network, RefreshCw, Route, Settings2, Sparkles, Waypoints } from "lucide-react";
import { useNavigate, useSearch } from "@tanstack/react-router";
import { useEffect, useState } from "react";

import { get } from "../api/client";
import { useClusters } from "../api/hooks";
import { Certificates } from "../components/ingress/Certificates";
import { Endpoints } from "../components/ingress/Endpoints";
import { Health } from "../components/ingress/Health";
import { ProfileEditor } from "../components/ingress/ProfileEditor";
import { Routing } from "../components/ingress/Routing";
import { type LiveResponse, MODES, type ProfileResponse } from "../components/ingress/types";
import { ClusterWizard } from "../components/setup/ClusterWizard";
import { FilterBar } from "../components/filters/FilterBar";
import { Badge, Button, Empty, ErrorNote, FadeSwitch, GroupedSections, IconButton, PageHeader, Skeleton, Status, VIEW_LIST, ViewToggle, groupedView, tabTrigger, usePageView } from "../components/ui";
import type { IngressSearch } from "../app/router";
import type { Cluster } from "../api/types";

const VIEWS = [VIEW_LIST, groupedView("cluster", "By cluster", Network)] as const;

type Tab = "endpoints" | "profile" | "routing" | "certificates" | "health";
const TAB_KEY = "forge.ingress.tab";
const CLUSTER_KEY = "forge.ingress.cluster";
const load = (k: string) => {
  try {
    return localStorage.getItem(k) ?? "";
  } catch {
    return "";
  }
};
const store = (k: string, v: string) => {
  try {
    localStorage.setItem(k, v);
  } catch {
    /* not remembered */
  }
};

/** Ingress per cluster: `/ingress?cluster=<id>&view=list|cluster`. */
export function Ingress() {
  const clusters = useClusters();
  const search = useSearch({ from: "/ingress" });
  const navigate = useNavigate();
  const patch = (p: Partial<IngressSearch>) => void navigate({ from: "/ingress", to: "/ingress", search: (s) => ({ ...s, ...p }), replace: true });
  const [view, setView] = usePageView("forge.ingress.view", VIEWS, search.view, (v) => patch({ view: v }));
  const list = clusters.data ?? [];
  // The URL's cluster, else the last one opened, else the first.
  const valid = (id: string | undefined) => (id && list.some((c) => c.id === id) ? id : undefined);
  const clusterId = valid(search.cluster) ?? valid(load(CLUSTER_KEY)) ?? list[0]?.id ?? "";
  const setClusterId = (id: string) => patch({ cluster: id || undefined });
  const [tab, setTab] = useState<Tab>(() => (["endpoints", "profile", "routing", "certificates", "health"].includes(load(TAB_KEY)) ? (load(TAB_KEY) as Tab) : "endpoints"));
  const [wizard, setWizard] = useState(false);

  useEffect(() => {
    if (clusterId) store(CLUSTER_KEY, clusterId);
  }, [clusterId]);
  useEffect(() => store(TAB_KEY, tab), [tab]);

  const profile = useQuery({
    queryKey: ["cluster-profile", clusterId],
    queryFn: () => get<ProfileResponse>(`/api/v1/clusters/${clusterId}/profile`),
    enabled: !!clusterId,
  });
  const live = useQuery({
    queryKey: ["cluster-live", clusterId],
    queryFn: () => get<LiveResponse>(`/api/v1/clusters/${clusterId}/live`),
    enabled: !!clusterId && !!profile.data?.profile,
    refetchInterval: tab === "routing" ? 10_000 : 30_000,
  });

  const p = profile.data?.profile;
  const members = profile.data?.members ?? [];
  const mode = MODES.find((m) => m.id === p?.ingress?.mode);
  const nameOf = (id: string) => members.find((m) => m.id === id)?.name ?? id.slice(0, 8);
  const vips = p?.ingress?.vrrp?.vips ?? [];

  return (
    <>
      <PageHeader
        eyebrow="Networking"
        title="Ingress"
        description="How traffic enters each cluster: Nginx on edge nodes in a standalone, active-standby or active-active layout, routing your domains to the services in the cluster."
        actions={
          <Button onClick={() => setWizard(true)}>
            <Sparkles className="size-4" /> Set up a cluster
          </Button>
        }
      />

      {clusters.error ? (
        <ErrorNote error={clusters.error} />
      ) : clusters.isLoading ? (
        <Skeleton />
      ) : !clusters.data?.length ? (
        <Empty icon={<Waypoints className="size-5" />} title="No clusters yet" action={<Button variant="primary" onClick={() => setWizard(true)}>Set up your first cluster</Button>}>
          A cluster groups nodes that share a network and an ingress. The wizard sets up the members, the cluster network and the Nginx layout in one go.
        </Empty>
      ) : (
        <>
          <FilterBar
            filters={[
              {
                id: "cluster",
                label: "Cluster",
                hidden: view === "cluster",
                value: clusterId ? [clusterId] : [],
                onChange: (v) => v[0] && setClusterId(v[0]),
                options: list.map((c) => ({ value: c.id, label: c.name })),
              },
            ]}
            view={<ViewToggle view={view} onChange={setView} modes={VIEWS} />}
          />
          {view === "cluster" ? (
            <FadeSwitch k="cluster" className="rounded-lg border border-line bg-panel shadow-[var(--shadow-card)]">
              <GroupedSections sections={list.map((c) => ({ id: c.id, title: c.name, items: [c] }))} label="Ingress by cluster">
                {(items) => items.map((c) => <ClusterEndpoints key={c.id} cluster={c} onOpen={() => patch({ cluster: c.id, view: "list" })} />)}
              </GroupedSections>
            </FadeSwitch>
          ) : (
          <FadeSwitch k="list">
          {/* Status strip */}
          <div className="mb-4 grid gap-3 rounded-lg border border-line bg-panel p-3 shadow-[var(--shadow-card)] sm:grid-cols-[auto_1fr_auto] sm:items-center">
            <div className="flex items-center gap-2">
              <span className="grid size-9 place-items-center rounded-lg bg-accent-soft text-accent">
                <Network className="size-5" />
              </span>
              <div>
                <div className="font-semibold">{mode ? mode.title : "No ingress profile"}</div>
                <div className="text-xs text-muted">
                  {p?.ingress ? `Nginx on ${p.ingress.edges.length} edge node${p.ingress.edges.length === 1 ? "" : "s"}` : "Configure it in the Profile tab"}
                </div>
              </div>
            </div>
            <div className="flex flex-wrap gap-2">
              {p?.ingress?.edges.map((e) => {
                const st = live.data?.edges[e.server_id]?.state;
                const applied = profile.data?.status?.edges?.[e.server_id];
                const held = st?.vips_held ?? [];
                return (
                  <div key={e.server_id} className="flex items-center gap-2 rounded-md border border-line px-2.5 py-1.5 text-xs" title={applied?.error ?? undefined}>
                    <Status value={st ? (st.nginx === "running" ? "online" : "failed") : applied?.status === "applied" ? "online" : applied?.status === "failed" ? "failed" : "pending"} />
                    <span className="font-medium">{nameOf(e.server_id)}</span>
                    {p.ingress!.mode === "active_standby" && <Badge tone={e.role === "primary" ? "accent" : "muted"}>{e.role}</Badge>}
                    {held.length > 0 && <Badge tone="ok">holds {held.join(", ")}</Badge>}
                    {st && st.keepalived !== "not-installed" && vips.length > 0 && <span className="text-faint">vrrp {st.keepalived}</span>}
                  </div>
                );
              })}
            </div>
            <div className="flex items-center gap-2">
              {profile.data?.status?.dirty && p && <Badge tone="warn">unapplied changes</Badge>}
              {vips.length > 0 && <Badge tone="accent">VIP {vips.join(", ")}</Badge>}
              <IconButton label="Refresh live state" onClick={() => void live.refetch()}>
                <RefreshCw className={`size-4 ${live.isFetching ? "animate-spin" : ""}`} />
              </IconButton>
            </div>
          </div>

          <div role="tablist" aria-label="Ingress sections" className="mb-4 flex gap-1 overflow-x-auto overflow-y-hidden border-b border-line">
            {(
              [
                ["endpoints", "Endpoints", Globe],
                ["profile", "Profile", Settings2],
                ["routing", "Routing", Route],
                ["certificates", "Certificates", FileKey2],
                ["health", "Health", Activity],
              ] as const
            ).map(([id, label, Icon]) => (
              <button key={id} role="tab" aria-selected={tab === id} data-state={tab === id ? "active" : "inactive"} onClick={() => setTab(id)} className={tabTrigger}>
                <Icon className="size-4" /> {label}
              </button>
            ))}
          </div>

          <FadeSwitch k={tab}>
          {profile.isLoading || !profile.data ? (
            <Skeleton />
          ) : tab === "endpoints" ? (
            <Endpoints clusterId={clusterId} members={members} hasProfile={!!p?.ingress} />
          ) : tab === "profile" ? (
            <ProfileEditor key={clusterId} clusterId={clusterId} data={profile.data} />
          ) : tab === "routing" ? (
            <Routing clusterId={clusterId} live={live.data} />
          ) : tab === "certificates" ? (
            <Certificates clusterId={clusterId} />
          ) : (
            <Health data={profile.data} live={live.data} clusterId={clusterId} />
          )}
          </FadeSwitch>
          </FadeSwitch>
          )}

          <details className="mt-6">
            <summary className="cursor-pointer text-xs text-muted">Standalone endpoints (not tied to a cluster)</summary>
            <div className="mt-3">
              <Endpoints />
            </div>
          </details>
        </>
      )}
      <ClusterWizard open={wizard} onOpenChange={setWizard} onDone={(id) => setClusterId(id)} />
    </>
  );
}

/** One cluster's section in the "By cluster" layout: its mode, edges and endpoints. */
function ClusterEndpoints({ cluster, onOpen }: { cluster: Cluster; onOpen: () => void }) {
  const profile = useQuery({
    queryKey: ["cluster-profile", cluster.id],
    queryFn: () => get<ProfileResponse>(`/api/v1/clusters/${cluster.id}/profile`),
  });
  const p = profile.data?.profile;
  const mode = MODES.find((m) => m.id === p?.ingress?.mode);
  const edges = p?.ingress?.edges?.length ?? 0;
  return (
    <div className="p-3">
      <div className="mb-2 flex min-w-0 flex-wrap items-center gap-2 text-xs text-muted">
        <span className="font-medium text-fg">{mode ? mode.title : profile.isLoading ? "…" : "No ingress profile"}</span>
        {p?.ingress && <span>· Nginx on {edges} edge node{edges === 1 ? "" : "s"}</span>}
        {profile.data?.status?.dirty && <Badge tone="warn">unapplied changes</Badge>}
        <button type="button" onClick={onOpen} className="ml-auto text-accent hover:underline">
          Profile, routing and certificates →
        </button>
      </div>
      <ErrorNote error={profile.error} />
      {profile.isLoading ? <Skeleton rows={2} /> : <Endpoints clusterId={cluster.id} members={profile.data?.members ?? []} hasProfile={!!p?.ingress} />}
    </div>
  );
}
