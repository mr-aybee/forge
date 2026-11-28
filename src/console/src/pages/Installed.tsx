import { useNavigate, useSearch } from "@tanstack/react-router";
import { PackageOpen, Server, Store } from "lucide-react";

import { useInstallations, useServers } from "../api/hooks";
import type { Installation } from "../api/types";
import type { InstalledSearch } from "../app/router";
import { one } from "../lib/search";
import { FilterBar } from "../components/filters/FilterBar";
import { PackageList } from "../components/PackageList";
import { Button, Empty, ErrorNote, GroupedSections, PageHeader, Panel, type Section, Skeleton, VIEW_LIST, ViewToggle, groupedView, usePageView } from "../components/ui";

const VIEWS = [VIEW_LIST, groupedView("nodes", "By node", Server)] as const;
type State = NonNullable<InstalledSearch["status"]>;
const matches = (i: Installation, s: State) => s === "all" || (s === "busy" ? i.status.endsWith("ing") : i.status === s);

/** Every App Store package on every node: `/apps-installed?node=&status=&q=&view=list|nodes`. */
export function Installed() {
  const navigate = useNavigate();
  const search = useSearch({ from: "/apps-installed" });
  const patch = (p: Partial<InstalledSearch>) => void navigate({ from: "/apps-installed", to: "/apps-installed", search: (s) => ({ ...s, ...p }), replace: true });
  const [view, setView] = usePageView("forge.installed.layout", VIEWS, search.view, (v) => patch({ view: v }));
  const inst = useInstallations();
  const { node, q = "" } = search;
  const status = search.status ?? "all";
  const nodes = useServers().data ?? [];
  const hostname = (id: string) => nodes.find((n) => n.id === id)?.hostname ?? id.slice(0, 8);
  const all = inst.data ?? [];
  const base = all.filter((i) => (!node || i.server_id === node) && `${i.package} ${i.version}`.toLowerCase().includes(q.toLowerCase()));
  const rows = base.filter((i) => matches(i, status));
  const groups = new Map<string, Installation[]>();
  for (const r of rows) groups.set(r.server_id, [...(groups.get(r.server_id) ?? []), r]);
  const sections: Section<Installation>[] = [...groups.entries()]
    .sort(([a], [b]) => hostname(a).localeCompare(hostname(b)))
    .map(([id, items]) => ({ id, title: hostname(id), meta: `${items.length} package${items.length === 1 ? "" : "s"}`, items }));
  // Nodes that have installations, for the filter.
  const nodeIds = [...new Set(all.map((i) => i.server_id))].sort((a, b) => hostname(a).localeCompare(hostname(b)));
  const count = (s: State) => base.filter((i) => matches(i, s)).length;
  return (
    <>
      <PageHeader
        eyebrow="Packages"
        title="Installed packages"
        description="Packages installed from the App Store, per node. Services read their variables from /etc/forge/pkgs/<package>.env."
        actions={
          <Button variant="primary" onClick={() => void navigate({ to: "/app-store", search: {} })}>
            <Store className="size-4" /> App Store
          </Button>
        }
      />
      <FilterBar
        status={{
          label: "Installation state",
          value: status,
          onChange: (v) => patch({ status: v === "all" ? undefined : v }),
          options: [
            { id: "all", label: "All", count: count("all") },
            { id: "installed", label: "Installed", count: count("installed") },
            { id: "busy", label: "In progress", count: count("busy") },
            { id: "failed", label: "Failed", count: count("failed") },
          ],
        }}
        search={{ value: q, onChange: (v) => patch({ q: v || undefined }), placeholder: "Search packages" }}
        filters={[{ id: "node", label: "Node", value: one(node), onChange: (v) => patch({ node: v[0] }), options: nodeIds.map((id) => ({ value: id, label: hostname(id), count: all.filter((i) => i.server_id === id).length })) }]}
        onClear={() => patch({ q: undefined, node: undefined })}
        view={<ViewToggle view={view} onChange={setView} modes={VIEWS} />}
      />
      <Panel title={`${rows.length} installation${rows.length === 1 ? "" : "s"}`}>
        <ErrorNote error={inst.error} />
        {inst.isLoading ? (
          <Skeleton />
        ) : inst.error ? null : rows.length === 0 ? (
          <Empty icon={<PackageOpen className="size-5" />} title={all.length ? "Nothing matches" : "Nothing installed yet"}>
            {all.length ? "Change or clear the filters above." : "Pick a package in the App Store and choose the nodes to install it on."}
          </Empty>
        ) : view === "list" ? (
          <div className="fade-in">
            <PackageList items={rows} />
          </div>
        ) : (
          <div className="fade-in">
            <GroupedSections sections={sections} label="Installations by node">
              {(items) => <PackageList items={items} showNode={false} />}
            </GroupedSections>
          </div>
        )}
      </Panel>
    </>
  );
}
