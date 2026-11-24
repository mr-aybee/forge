import { Link, useNavigate, useSearch } from "@tanstack/react-router";
import { ClusterBadge, imageToDataUrl, initials } from "../components/ClusterBadge";
import { Pencil, Plus, ServerCog, Sparkles, Trash2 } from "lucide-react";
import { useState } from "react";

import { useAllEnvironments, useClusters, useDeleteCluster, useSaveCluster } from "../api/hooks";
import type { Cluster, ClusterMode } from "../api/types";
import { errorText, useToast } from "../app/toast";
import { ClusterWizard } from "../components/setup/ClusterWizard";
import { ModePicker } from "../components/ClusterModePicker";
import { usePinAction } from "../components/Pins";
import { Badge, Button, Confirm, Empty, ErrorNote, FadeSwitch, Field, IconButton, Input, Modal, PageHeader, Panel, RowMenu, Select, Skeleton, Table, Td, TextArea, VIEW_CARDS, VIEW_LIST, ViewToggle, usePageView } from "../components/ui";
import { FilterBar } from "../components/filters/FilterBar";
import type { ClustersSearch } from "../app/router";

const VIEWS = [VIEW_LIST, VIEW_CARDS] as const;
type Health = "healthy" | "degraded" | "empty";

export const modeLabel = (c: Pick<Cluster, "mode">) => (c.mode === "swarm" ? "Swarm" : "Default");

const healthOf = (c: Cluster): Health => (!c.node_count ? "empty" : (c.nodes_online ?? 0) === c.node_count ? "healthy" : "degraded");

export function ClusterForm({ initial, onDone }: { initial?: Cluster; onDone: () => void }) {
    const envs = useAllEnvironments();
  const save = useSaveCluster();
  const toast = useToast();
  const [f, setF] = useState({
    name: initial?.name ?? "",
    description: initial?.description ?? "",
    environment_id: initial?.environment_id ?? "",
    abbr: initial?.abbr ?? "",
    icon: initial?.icon ?? "",
    mode: initial?.mode ?? ("default" as ClusterMode),
  });
  return (
    <form
      className="space-y-4"
      onSubmit={(e) => {
        e.preventDefault();
        save.mutate(
          { id: initial?.id, name: f.name, description: f.description, environment_id: f.environment_id || null, abbr: f.abbr, icon: f.icon, ...(initial ? {} : { mode: f.mode }) },
          {
            onSuccess: (c) => {
              toast(initial ? `Cluster “${c.name}” updated` : `Cluster “${c.name}” created`);
              onDone();
            },
            onError: (err) => toast(errorText(err), "error"),
          },
        );
      }}
    >
      <Field label="Name" hint="Letters, digits, spaces, - _ .">
        <Input required autoFocus maxLength={63} value={f.name} onChange={(e) => setF({ ...f, name: e.target.value })} placeholder="edge-eu" />
      </Field>
      <Field label="Mode" hint={initial ? undefined : "Cannot be changed after the cluster is created."}>
        <ModePicker value={f.mode} locked={!!initial} onChange={(mode) => setF({ ...f, mode })} />
      </Field>
      <Field label="Badge" hint="Shown next to the cluster name. Upload a PNG, JPEG, WebP or SVG (up to 128 KB), or set up to 4 letters.">
        <div className="flex flex-wrap items-center gap-3">
          <ClusterBadge c={{ name: f.name || "?", abbr: f.abbr, icon: f.icon || null }} size={44} />
          <Input className="!w-24 shrink-0 text-center font-semibold uppercase" maxLength={4} value={f.abbr} onChange={(e) => setF({ ...f, abbr: e.target.value.replace(/[^a-z0-9]/gi, "").toUpperCase() })} placeholder={initials({ name: f.name || "?" })} aria-label="Abbreviation" />
          <label className="cursor-pointer rounded-md border border-line px-3 py-1.5 text-[13px] hover:border-accent">
            Upload image
            <input
              type="file"
              accept="image/png,image/jpeg,image/webp,image/gif,image/svg+xml"
              className="sr-only"
              onChange={async (e) => {
                const file = e.target.files?.[0];
                e.target.value = "";
                if (!file) return;
                try {
                  const url = await imageToDataUrl(file);
                  if (url.length > 128 * 1024) toast("That image is larger than 128 KB", "error");
                  else setF((cur) => ({ ...cur, icon: url }));
                } catch {
                  toast("Could not read that image", "error");
                }
              }}
            />
          </label>
          {f.icon && (
            <Button type="button" variant="ghost" onClick={() => setF({ ...f, icon: "" })}>
              Remove image
            </Button>
          )}
        </div>
      </Field>
      <Field label="Description">
        <TextArea rows={2} maxLength={500} value={f.description} onChange={(e) => setF({ ...f, description: e.target.value })} className="font-sans text-[13px]" />
      </Field>
      <Field label="Environment" hint="Optional: the stage this cluster serves.">
        <Select value={f.environment_id} onChange={(e) => setF({ ...f, environment_id: e.target.value })}>
          <option value="">None</option>
          {envs.data?.map((e) => (
            <option key={e.id} value={e.id}>
              {e.name}
            </option>
          ))}
        </Select>
      </Field>
      <div className="flex justify-end">
        <Button variant="primary" disabled={save.isPending}>
          {initial ? "Save changes" : "Create cluster"}
        </Button>
      </div>
    </form>
  );
}

export function Clusters() {
    const clusters = useClusters();
  const envs = useAllEnvironments();
  const del = useDeleteCluster();
  const toast = useToast();
  const [editing, setEditing] = useState<Cluster | "new">();
  const [deleting, setDeleting] = useState<Cluster>();
  const [delErr, setDelErr] = useState("");
  const [wizard, setWizard] = useState(false);
  const blocking = deleting ? (envs.data ?? []).filter((n) => n.cluster_id === deleting.id) : [];
  const search = useSearch({ from: "/clusters" });
  const navigate = useNavigate();
  const patch = (p: Partial<ClustersSearch>) => void navigate({ from: "/clusters", to: "/clusters", search: (s) => ({ ...s, ...p }), replace: true });
  const [view, setView] = usePageView("forge.clusters.view", VIEWS, search.view, (v) => patch({ view: v }));
  const status = search.status ?? "all";
  const q = search.q ?? "";
  const all = clusters.data ?? [];
  const needle = q.trim().toLowerCase();
  const matchQ = all.filter((c) => `${c.name} ${c.description ?? ""}`.toLowerCase().includes(needle));
  const shown = matchQ.filter((c) => status === "all" || healthOf(c) === status);
  const n = (h: Health) => matchQ.filter((c) => healthOf(c) === h).length;
  const pinAction = usePinAction();
  const actions = (c: Cluster) => [
    pinAction({ kind: "cluster", id: c.id, label: c.name }),
    { label: "Edit", icon: Pencil, onSelect: () => setEditing(c) },
    { label: "Delete", icon: Trash2, danger: true, onSelect: () => setDeleting(c) },
  ];

  return (
    <>
      <PageHeader
        eyebrow="Infrastructure"
        title="Clusters"
        description="A cluster is a named group of nodes. Services and sites run on a cluster; permissions can be granted per cluster."
        actions={
          <>
            <Button onClick={() => setEditing("new")}>
              <Plus className="size-4" /> Empty cluster
            </Button>
            <Button variant="primary" onClick={() => setWizard(true)}>
              <Sparkles className="size-4" /> Set up a cluster
            </Button>
          </>
        }
      />
      {all.length > 0 && (
        <FilterBar
          status={{
            label: "Cluster health",
            value: status,
            onChange: (v) => patch({ status: v === "all" ? undefined : v }),
            options: [
              { id: "all", label: "All", count: matchQ.length },
              { id: "healthy", label: "Healthy", count: n("healthy"), title: "Every node online" },
              { id: "degraded", label: "Degraded", count: n("degraded"), title: "Some nodes offline" },
              { id: "empty", label: "Empty", count: n("empty"), title: "No nodes" },
            ],
          }}
          search={{ value: q, onChange: (v) => patch({ q: v || undefined }), placeholder: "Search clusters" }}
          onClear={() => patch({ q: undefined })}
          view={<ViewToggle view={view} onChange={setView} modes={VIEWS} />}
        />
      )}
      {clusters.error ? (
        <ErrorNote error={clusters.error} />
      ) : clusters.isLoading ? (
        <Skeleton rows={3} />
      ) : all.length === 0 ? (
        <div className="rounded-lg border border-line bg-panel">
          <Empty icon={<ServerCog className="size-5" />} title="No clusters yet" action={<Button variant="primary" onClick={() => setEditing("new")}>Create your first cluster</Button>}>
            Group nodes by region, purpose or customer — for example “edge-eu” or “windows-workers”.
          </Empty>
        </div>
      ) : shown.length === 0 ? (
        <p className="rounded-lg border border-line bg-panel px-4 py-6 text-center text-xs text-muted">No clusters match these filters.</p>
      ) : view === "list" ? (
        <FadeSwitch k="list">
          <Panel>
            <Table head={["Cluster", "Nodes online", "Environment", "Description", ""]}>
              {shown.map((c) => {
                const total = c.node_count ?? 0;
                const online = c.nodes_online ?? 0;
                const env = envs.data?.find((e) => e.id === c.environment_id);
                return (
                  <tr key={c.id} className="hover:bg-panel-2/60">
                    <Td className="py-1.5">
                      <Link to="/clusters/$clusterId" params={{ clusterId: c.id }} className="flex min-w-0 items-center gap-2 font-medium hover:text-accent">
                        <ClusterBadge c={c} size={22} />
                        <span className="truncate" title={c.name}>
                          {c.name}
                        </span>
                        {c.mode === "swarm" && <Badge tone="accent">Swarm</Badge>}
                      </Link>
                    </Td>
                    <Td className="py-1.5">
                      <Link to="/nodes" search={{ cluster: c.id }} className="inline-flex items-center gap-1.5 font-mono text-[11px] tabular-nums hover:text-accent" title="Open this cluster's nodes">
                        <span className={`size-1.5 rounded-full ${healthOf(c) === "healthy" ? "bg-ok" : healthOf(c) === "degraded" ? "bg-warn" : "bg-faint"}`} aria-hidden />
                        {total ? `${online}/${total}` : "—"}
                      </Link>
                    </Td>
                    <Td className="py-1.5 text-xs text-muted">{env?.name ?? "—"}</Td>
                    <Td className="max-w-[420px] py-1.5">
                      <span className="block truncate text-xs text-muted" title={c.description || undefined}>
                        {c.description || "—"}
                      </span>
                    </Td>
                    <Td className="w-10 py-1 text-right">
                      <RowMenu actions={actions(c)} label={`Actions for ${c.name}`} />
                    </Td>
                  </tr>
                );
              })}
            </Table>
          </Panel>
        </FadeSwitch>
      ) : (
        <FadeSwitch k="cards" className="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
          {shown.map((c) => {
            const total = c.node_count ?? 0;
            const online = c.nodes_online ?? 0;
            const pct = total ? Math.round((online / total) * 100) : 0;
            const env = envs.data?.find((e) => e.id === c.environment_id);
            return (
              <article key={c.id} className="group rounded-lg border border-line bg-panel p-4 shadow-[var(--shadow-card)] transition hover:border-accent/50">
                <div className="flex items-start justify-between gap-2">
                  <Link to="/clusters/$clusterId" params={{ clusterId: c.id }} className="min-w-0">
                    <div className="flex items-center gap-2">
                      <span className="btn-primary grid size-8 place-items-center rounded-md text-white">
                        <ServerCog className="size-4" />
                      </span>
                      <span className="truncate font-semibold hover:text-accent">{c.name}</span>
                    </div>
                  </Link>
                  <div className="flex opacity-60 transition group-hover:opacity-100">
                    <IconButton label="Edit cluster" onClick={() => setEditing(c)}>
                      <Pencil className="size-4" />
                    </IconButton>
                    <IconButton label="Delete cluster" onClick={() => setDeleting(c)}>
                      <Trash2 className="size-4" />
                    </IconButton>
                  </div>
                </div>
                <p className="mt-2 line-clamp-2 min-h-8 text-xs text-muted">{c.description || "No description"}</p>
                <div className="mt-3 flex items-center justify-between text-xs">
                  <span className="text-muted">
                    {online}/{total} nodes online
                  </span>
                  {env && <span className="rounded bg-panel-2 px-1.5 py-0.5 text-muted">{env.name}</span>}
                </div>
                <div className="mt-1.5 h-1.5 overflow-hidden rounded-full bg-panel-2" aria-label={`${pct}% online`}>
                  <div className={`h-full rounded-full ${pct === 100 ? "bg-ok" : "btn-primary"}`} style={{ width: `${pct}%` }} />
                </div>
              </article>
            );
          })}
        </FadeSwitch>
      )}

      <Modal open={!!editing} onOpenChange={(o) => !o && setEditing(undefined)} title={editing === "new" ? "New cluster" : "Edit cluster"}>
        {editing && <ClusterForm initial={editing === "new" ? undefined : editing} onDone={() => setEditing(undefined)} />}
      </Modal>
      <Confirm
        open={!!deleting}
        onOpenChange={(o) => {
          if (!o) {
            setDeleting(undefined);
            setDelErr("");
          }
        }}
        title={`Delete cluster “${deleting?.name ?? ""}”?`}
        disabled={blocking.length > 0}
        body={
          <div className="space-y-2">
            {blocking.length > 0 ? (
              <>
                <p>
                  Every environment lives in a cluster. Move this cluster's {blocking.length} environment{blocking.length === 1 ? "" : "s"} to another cluster first:
                </p>
                <ul className="flex flex-wrap gap-1">
                  {blocking.map((n) => (
                    <li key={n.id} className="max-w-full min-w-0">
                      <Link to="/projects" search={{ sel: `env:${n.id}` }} className="block truncate rounded border border-line px-1.5 py-px font-mono text-[11px] text-fg hover:border-accent hover:text-accent" title={`Open ${n.name} in Projects`}>
                        {n.name}
                      </Link>
                    </li>
                  ))}
                </ul>
                <p className="text-xs text-muted">Open each one in Projects and use Move to cluster.</p>
              </>
            ) : (
              <p>Its nodes stay enrolled and become unassigned. Nothing is uninstalled from the machines.</p>
            )}
            {delErr && (
              <p role="alert" className="rounded-md bg-bad-soft px-2.5 py-1.5 text-xs text-bad">
                {delErr}
              </p>
            )}
          </div>
        }
        confirmLabel="Delete cluster"
        busy={del.isPending}
        onConfirm={() => {
          if (!deleting) return;
          setDelErr("");
          del.mutate(deleting.id, {
            onSuccess: () => {
              toast(`Cluster “${deleting.name}” deleted`);
              setDeleting(undefined);
            },
            // 409 names the environments that still live here.
            onError: (e) => setDelErr(errorText(e)),
          });
        }}
      />
      <ClusterWizard open={wizard} onOpenChange={setWizard} />
    </>
  );
}
