import { Link, useNavigate, useSearch } from "@tanstack/react-router";
import { AppWindow, Boxes, FolderInput, Layers, Lock, Plus } from "lucide-react";
import { useState } from "react";

import { useAllEnvironments, useProjects, useServices, useClusters } from "../api/hooks";
import type { ServicesSearch } from "../app/router";
import { one } from "../lib/search";
import { AdoptEnvironmentDialog } from "../components/AdoptEnvironmentDialog";
import { FilterBar } from "../components/filters/FilterBar";
import { ServiceTable } from "../components/ServiceTable";
import { Badge, Button, Empty, ErrorNote, GroupedSections, PageHeader, Panel, type Section, Skeleton, VIEW_LIST, ViewToggle, groupedView, usePageView } from "../components/ui";
import type { Service } from "../api/types";

const VIEWS = [VIEW_LIST, groupedView("env", "By environment", Layers)] as const;

/**
 * Every service across all projects and environments.
 * Route `/services?cluster=&project=&env=&q=&view=`.
 */
export function Services() {
  const search = useSearch({ from: "/services" });
  const navigate = useNavigate();
  const patch = (p: Partial<ServicesSearch>) => void navigate({ from: "/services", to: "/services", search: (s) => ({ ...s, ...p }), replace: true });
  const [view, setView] = usePageView("forge.services.view", VIEWS, search.view, (v) => patch({ view: v }));
  const apps = useServices();
  const nsQ = useAllEnvironments();
  const environments = nsQ.data ?? [];
  const groups = useProjects().data ?? [];
  const clusters = useClusters().data ?? [];
  const [adopting, setAdopting] = useState(false);
  const { cluster, project, env, q = "" } = search;
  const create = () => void navigate({ to: "/services/new", search: { cluster, environment: env } });

  const nsOf = (id: string) => environments.find((n) => n.id === id);
  const needle = q.trim().toLowerCase();
  const all = apps.data ?? [];
  const rows = all.filter((a) => {
    const ns = nsOf(a.environment_id);
    return (
      (!cluster || ns?.cluster_id === cluster) &&
      (!project || ns?.project_id === project) &&
      (!env || a.environment_id === env) &&
      `${a.name} ${a.runtime} ${a.spec?.image ?? ""} ${ns?.name ?? ""}`.toLowerCase().includes(needle)
    );
  });
  const total = all.length;
  // Environment choices follow the cluster and project filters.
  const envChoices = environments.filter((n) => (!cluster || n.cluster_id === cluster) && (!project || n.project_id === project));
  const unassigned = environments.filter((n) => !n.cluster_id);
  const clusterName = (id: string | null | undefined) => clusters.find((c) => c.id === id)?.name;

  const sections: Section<Service>[] = envChoices
    .map((n) => {
      const g = groups.find((x) => x.id === n.project_id);
      return {
        id: n.id,
        title: (
          <span className="inline-flex min-w-0 items-center gap-1.5">
            <Layers className="size-3.5 shrink-0 text-accent" aria-hidden />
            <Link to="/projects" search={{ sel: `env:${n.id}` }} className="truncate hover:text-accent" title={`Environment ${n.name}`}>
              {n.name}
            </Link>
            {n.protected && (
              <Badge tone="warn">
                <Lock className="mr-1 size-3" /> protected
              </Badge>
            )}
            {!cluster && clusterName(n.cluster_id) && <span className="truncate text-xs font-normal text-faint">· {clusterName(n.cluster_id)}</span>}
          </span>
        ),
        actions: g ? (
          <Link to="/projects" search={{ sel: `project:${g.id}` }} className="inline-flex max-w-[200px] min-w-0 shrink-0 items-center gap-1 text-xs text-muted hover:text-accent" title={`Project ${g.name}`}>
            <AppWindow className="size-3.5 shrink-0" aria-hidden />
            <span className="truncate">{g.name}</span>
          </Link>
        ) : undefined,
        items: rows.filter((a) => a.environment_id === n.id),
      };
    })
    .sort((a, b) => b.items.length - a.items.length);

  return (
    <>
      <PageHeader
        eyebrow="Projects"
        title="Services"
        description="Every service across all projects and environments: APIs, UIs, databases and containers, with the nodes they run on."
        actions={
          <>
            {cluster && (
              <Button disabled={!unassigned.length} title={unassigned.length ? "Move an unassigned environment into this cluster" : "No unassigned environments"} onClick={() => setAdopting(true)}>
                <FolderInput className="size-4" /> Add existing environment
              </Button>
            )}
            <Button variant="primary" onClick={create}>
              <Plus className="size-4" /> New service
            </Button>
          </>
        }
      />
      <FilterBar
        search={{ value: q, onChange: (v) => patch({ q: v || undefined }), placeholder: "Search services" }}
        filters={[
          {
            id: "cluster",
            label: "Cluster",
            value: one(cluster),
            onChange: (v) => patch({ cluster: v[0], env: undefined }),
            options: clusters.map((c) => ({ value: c.id, label: c.name })),
          },
          {
            id: "project",
            label: "Project",
            value: one(project),
            onChange: (v) => patch({ project: v[0], env: undefined }),
            hidden: !groups.length,
            options: groups.map((g) => ({ value: g.id, label: g.name })),
          },
          {
            id: "env",
            label: "Environment",
            value: one(env),
            onChange: (v) => patch({ env: v[0] }),
            options: envChoices.map((n) => ({ value: n.id, label: cluster ? n.name : `${n.name}${clusterName(n.cluster_id) ? ` · ${clusterName(n.cluster_id)}` : ""}` })),
          },
        ]}
        onClear={() => patch({ q: undefined, cluster: undefined, project: undefined, env: undefined })}
        view={<ViewToggle view={view} onChange={setView} modes={VIEWS} />}
      />
      <Panel title={apps.isLoading ? "Services" : `${rows.length} service${rows.length === 1 ? "" : "s"}${rows.length !== total ? ` of ${total}` : ""}`}>
        <ErrorNote error={apps.error ?? nsQ.error} />
        {apps.isLoading ? (
          <Skeleton />
        ) : apps.error ? null : total === 0 ? (
          <Empty
            icon={<Boxes className="size-5" />}
            title="No services yet"
            action={
              <Button variant="primary" onClick={create}>
                New service
              </Button>
            }
          >
            A service is one part of a project in one environment: an API as a systemd unit or a container, on one node or as an active/standby pair.
          </Empty>
        ) : rows.length === 0 ? (
          <p className="px-4 py-6 text-center text-xs text-muted">No services match these filters.</p>
        ) : view === "env" ? (
          <div className="fade-in">
            <GroupedSections sections={sections} label="Services by environment">
              {(list) => <ServiceTable apps={list} />}
            </GroupedSections>
          </div>
        ) : (
          <div className="fade-in">
            <ServiceTable apps={rows} environments={environments} />
          </div>
        )}
      </Panel>
      {adopting && cluster && <AdoptEnvironmentDialog onClose={() => setAdopting(false)} clusterId={cluster} candidates={unassigned} />}
    </>
  );
}
