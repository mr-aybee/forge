import { Link, useNavigate, useSearch } from "@tanstack/react-router";
import { Container, FileCode2, Layers, Plus, RefreshCw } from "lucide-react";
import { useMemo } from "react";

import { useAllEnvironments, useClusters } from "../api/hooks";
import { useStacks } from "../api/stacks";
import { StackTable } from "../components/stacks/StackList";
import { Button, Empty, ErrorNote, IconButton, PageHeader, Panel, Skeleton } from "../components/ui";
import { FilterBar } from "../components/filters/FilterBar";
import { one } from "../lib/search";

/** Compose stacks per environment, with an environment rail grouped by cluster. */
export function Stacks() {
  const search = useSearch({ from: "/stacks" });
  const { ns, cluster, kind } = search;
  const q = search.q ?? "";
  const navigate = useNavigate();
  const environments = useAllEnvironments();
  const clusters = useClusters().data ?? [];
  const all = useStacks();
  const patch = (p: Partial<typeof search>) => void navigate({ from: "/stacks", to: "/stacks", search: (s) => ({ ...s, ...p }), replace: true });

  const entries = all.data ?? [];
  const count = (id?: string) => (id ? entries.filter((e) => e.stack.environment_id === id).length : entries.length);
  const focused = (environments.data ?? []).find((n) => n.id === ns);
  const needle = q.trim().toLowerCase();
  const clusterOf = (nsId: string) => (environments.data ?? []).find((n) => n.id === nsId)?.cluster_id;
  const rows = entries.filter(
    (e) =>
      (!ns || e.stack.environment_id === ns) &&
      (!cluster || clusterOf(e.stack.environment_id) === cluster) &&
      (!kind || e.stack.kind === kind) &&
      (!needle || `${e.stack.name} ${e.stack.kind}`.toLowerCase().includes(needle)),
  );
  const kinds = [...new Set(entries.map((e) => e.stack.kind))].sort();

  const groups = useMemo(() => {
    const list = environments.data ?? [];
    const by = new Map<string, typeof list>();
    for (const n of list) {
      const key = n.cluster_id ?? "";
      by.set(key, [...(by.get(key) ?? []), n]);
    }
    return [...by.entries()].map(([cid, items]) => ({ id: cid, name: clusters.find((c) => c.id === cid)?.name ?? "Unassigned", items: items.sort((a, b) => a.name.localeCompare(b.name)) }));
  }, [environments.data, clusters]);

  const pick = (id?: string) => patch({ ns: id });
  const railItem = (id: string | undefined, label: string) => {
    const active = (ns ?? "") === (id ?? "");
    return (
      <button
        key={id ?? "all"}
        onClick={() => pick(id)}
        aria-current={active ? "page" : undefined}
        className={`flex w-full min-w-0 items-center justify-between gap-2 rounded px-2 py-1 text-left text-[13px] ${active ? "bg-accent-soft text-accent" : "text-muted hover:bg-panel-2 hover:text-fg"}`}
      >
        <span className="truncate" title={label}>
          {label}
        </span>
        <span className="font-mono text-[11px] tabular-nums">{count(id) || "—"}</span>
      </button>
    );
  };

  return (
    <>
      <PageHeader
        eyebrow={focused ? `Stacks / ${focused.name}` : "Packages"}
        title="Stacks"
        description="Docker Compose stacks and single containers per environment: their revisions, the nodes they run on and each service's state."
        actions={
          <>
            <Button onClick={() => navigate({ to: "/stacks/new-container", search: { environment: ns } })}>
              <Container className="size-4" /> Create container
            </Button>
            <Button variant="primary" onClick={() => navigate({ to: "/stacks/new", search: { environment: ns } })}>
              <FileCode2 className="size-4" /> Create from Compose
            </Button>
          </>
        }
      />
      <div className="grid gap-4 md:grid-cols-[200px_minmax(0,1fr)]">
        <nav aria-label="Environments" className="min-w-0 space-y-3 md:sticky md:top-2 md:self-start">
          {railItem(undefined, "All environments")}
          {environments.isLoading ? (
            <Skeleton rows={3} />
          ) : (
            groups.map((g) => (
              <div key={g.id || "none"} className="min-w-0">
                <div className="mb-0.5 truncate px-2 text-[11px] font-semibold tracking-wider text-faint uppercase" title={g.name}>
                  {g.id ? (
                    <Link to="/clusters/$clusterId" params={{ clusterId: g.id }} className="hover:text-accent">
                      {g.name}
                    </Link>
                  ) : (
                    g.name
                  )}
                </div>
                {g.items.map((n) => railItem(n.id, n.name))}
              </div>
            ))
          )}
        </nav>
        <div className="min-w-0">
        <FilterBar
          search={{ value: q, onChange: (v) => patch({ q: v || undefined }), placeholder: "Search stacks" }}
          filters={[
            { id: "cluster", label: "Cluster", value: one(cluster), onChange: (v) => patch({ cluster: v[0] }), hidden: !clusters.length, options: clusters.map((c) => ({ value: c.id, label: c.name })) },
            { id: "kind", label: "Kind", value: one(kind), onChange: (v) => patch({ kind: v[0] }), hidden: kinds.length < 2 && !kind, options: kinds.map((k) => ({ value: k, label: k })) },
          ]}
          onClear={() => patch({ q: undefined, cluster: undefined, kind: undefined })}
          view={
            <IconButton label="Refresh" onClick={() => void all.refetch()}>
              <RefreshCw className={`size-4 ${all.isFetching ? "animate-spin" : ""}`} />
            </IconButton>
          }
        />
        <Panel title={`${rows.length} stack${rows.length === 1 ? "" : "s"}${focused ? ` in ${focused.name}` : ""}`}>
          <ErrorNote error={all.error} />
          {all.isLoading ? (
            <Skeleton />
          ) : (
            <StackTable
              rows={rows}
              showEnvironment={!ns}
              empty={
                (needle || cluster || kind) && entries.length ? (
                  <p className="px-4 py-6 text-center text-xs text-muted">No stacks match these filters.</p>
                ) : (
                  <Empty
                    icon={<Layers className="size-5" />}
                    title={focused ? `No stacks in ${focused.name}` : "No stacks yet"}
                    action={
                      <Button variant="primary" onClick={() => navigate({ to: "/stacks/new", search: { environment: ns } })}>
                        <Plus className="size-4" /> Create from Compose
                      </Button>
                    }
                  >
                    Paste a docker-compose file to run it on a node, or create a single container from a node's Containers tab.
                  </Empty>
                )
              }
            />
          )}
        </Panel>
        </div>
      </div>
    </>
  );
}
