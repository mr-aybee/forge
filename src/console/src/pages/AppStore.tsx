import { Link, useSearch } from "@tanstack/react-router";
import { Activity, Box, Boxes, Code2, Database, GitBranch, Globe, HardDrive, LayoutGrid, List, MessagesSquare, Network, Sparkles, Wrench, type LucideIcon } from "lucide-react";
import { useMemo, useState } from "react";

import { useCatalog, useInstallations } from "../api/hooks";
import type { CatalogPackage, PackageCategory } from "../api/types";
import { InstallWizard } from "../components/InstallWizard";
import { Badge, Button, Empty, ErrorNote, PageHeader, SearchInput, Skeleton } from "../components/ui";

export const CATEGORIES: { id: PackageCategory; label: string; icon: LucideIcon }[] = [
  { id: "runtimes", label: "Runtimes", icon: Code2 },
  { id: "web", label: "Web servers", icon: Globe },
  { id: "databases", label: "Databases", icon: Database },
  { id: "storage", label: "Object storage", icon: HardDrive },
  { id: "containers", label: "Containers", icon: Boxes },
  { id: "messaging", label: "Messaging", icon: MessagesSquare },
  { id: "monitoring", label: "Monitoring", icon: Activity },
  { id: "devops", label: "DevOps", icon: GitBranch },
  { id: "dns", label: "DNS", icon: Network },
  { id: "tools", label: "Tools", icon: Wrench },
];
export const categoryIcon = (c: PackageCategory) => CATEGORIES.find((x) => x.id === c)?.icon ?? Box;

const VIEW_KEY = "forge.appstore.view";

function RailItem({ active, onClick, icon: Icon, label, n }: { active: boolean; onClick: () => void; icon: LucideIcon; label: string; n: number }) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-current={active ? "page" : undefined}
      className={`flex shrink-0 items-center gap-2 rounded-lg px-2.5 py-1.5 text-left text-[13px] transition lg:w-full ${active ? "bg-accent-soft font-medium text-accent" : "text-muted hover:bg-panel-2 hover:text-fg"}`}
    >
      <Icon className="size-4 shrink-0" />
      <span className="flex-1 truncate">{label}</span>
      <span className={`rounded px-1.5 text-[10px] tabular-nums ${active ? "bg-panel text-accent" : "bg-panel-2 text-faint"}`}>{n}</span>
    </button>
  );
}

const KIND: Record<CatalogPackage["kind"], string> = { native: "Native", container: "Container", plugin: "Plugin" };

export function AppStore() {
  const search = useSearch({ from: "/app-store" });
  const catalog = useCatalog();
  const installations = useInstallations().data ?? [];
  const [q, setQ] = useState("");
  const [cat, setCat] = useState<PackageCategory | "all">("all");
  const [chosen, setChosen] = useState<CatalogPackage | null>(null);
  const [view, setViewState] = useState<"list" | "cards">(() => {
    try {
      return localStorage.getItem(VIEW_KEY) === "cards" ? "cards" : "list";
    } catch {
      return "list";
    }
  });
  const setView = (v: "list" | "cards") => {
    setViewState(v);
    try {
      localStorage.setItem(VIEW_KEY, v);
    } catch {
      /* private mode: keep it for this visit */
    }
  };
  const packages = catalog.data?.packages ?? [];

  const shown = useMemo(
    () =>
      packages.filter(
        (p) => (cat === "all" || p.category === cat) && `${p.name} ${p.summary} ${p.id}`.toLowerCase().includes(q.toLowerCase()),
      ),
    [packages, cat, q],
  );
  const count = (id: string) => new Set(installations.filter((i) => i.package === id && i.status === "installed").map((i) => i.server_id)).size;

  return (
    <>
      <PageHeader
        eyebrow="Packages"
        title="App Store"
        description="Install runtimes, databases, web servers and platform plugins on any node — one or many at a time."
      />
      <div className="grid gap-5 lg:grid-cols-[200px_minmax(0,1fr)]">
        {/* Category rail */}
        <nav aria-label="Categories" className="lg:sticky lg:top-4 lg:self-start">
          <div className="flex gap-1 overflow-x-auto pb-1 lg:flex-col lg:overflow-visible lg:pb-0">
            <RailItem active={cat === "all"} onClick={() => setCat("all")} icon={Box} label="All packages" n={packages.length} />
            {CATEGORIES.filter((c) => packages.some((p) => p.category === c.id)).map((c) => (
              <RailItem key={c.id} active={cat === c.id} onClick={() => setCat(c.id)} icon={c.icon} label={c.label} n={packages.filter((p) => p.category === c.id).length} />
            ))}
          </div>
        </nav>

        <div className="min-w-0">
          <div className="mb-3 flex flex-wrap items-center gap-2">
            <SearchInput value={q} onChange={setQ} placeholder="Search packages" />
            <span className="text-xs text-faint">
              {shown.length} package{shown.length === 1 ? "" : "s"}
            </span>
            <div className="ml-auto inline-flex rounded-lg border border-line bg-panel p-0.5" role="group" aria-label="View">
              {(
                [
                  ["list", List, "List"],
                  ["cards", LayoutGrid, "Cards"],
                ] as const
              ).map(([v, Icon, label]) => (
                <button
                  key={v}
                  type="button"
                  aria-pressed={view === v}
                  onClick={() => setView(v)}
                  className={`inline-flex h-7 items-center gap-1.5 rounded-md px-2.5 text-xs transition ${view === v ? "bg-accent-soft font-medium text-accent" : "text-muted hover:text-fg"}`}
                >
                  <Icon className="size-3.5" /> {label}
                </button>
              ))}
            </div>
          </div>
          <ErrorNote error={catalog.error} />
          {catalog.isLoading ? (
            <Skeleton rows={6} />
          ) : shown.length === 0 ? (
            <Empty title="No packages match" />
          ) : view === "list" ? (
            <div key="list" className="view-enter overflow-hidden rounded-xl border border-line bg-panel">
              {shown.map((p) => {
                const Icon = categoryIcon(p.category);
                const n = count(p.id);
                return (
                  <div key={p.id} className="flex items-center gap-3 border-b border-line px-3 py-2 last:border-b-0 hover:bg-panel-2/60">
                    <span className="grid size-8 shrink-0 place-items-center rounded-md bg-accent-soft text-accent">
                      <Icon className="size-4" />
                    </span>
                    <div className="min-w-0 flex-1">
                      <div className="flex items-center gap-2">
                        <Link to="/app-store/$packageId" params={{ packageId: p.id }} className="truncate font-medium hover:text-accent">
                          {p.name}
                        </Link>
                        {n > 0 && <Badge tone="ok">on {n} node{n === 1 ? "" : "s"}</Badge>}
                        {p.os.includes("windows") && <Badge>Windows too</Badge>}
                      </div>
                      <div className="truncate text-xs text-muted">{p.summary}</div>
                    </div>
                    <div className="hidden w-44 shrink-0 text-[11px] text-faint md:block">
                      <div className="truncate">{p.versions.length > 1 ? `Versions ${p.versions.join(", ")}` : `Version ${p.versions[0]}`}</div>
                      {p.ports.length > 0 && <div className="truncate">ports {p.ports.join(", ")}</div>}
                    </div>
                    <span className="hidden w-28 shrink-0 truncate text-[11px] text-faint lg:block" title={p.license}>
                      {p.license}
                    </span>
                    <div className="flex shrink-0 gap-1.5">
                      <Link to="/app-store/$packageId" params={{ packageId: p.id }}>
                        <Button variant="ghost" className="h-7 px-2 text-xs">
                          Details
                        </Button>
                      </Link>
                      <Button variant="primary" className="h-7 px-2.5 text-xs" onClick={() => setChosen(p)}>
                        Install
                      </Button>
                    </div>
                  </div>
                );
              })}
            </div>
          ) : (
            <div key="cards" className="view-enter grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
              {shown.map((p) => {
                const Icon = categoryIcon(p.category);
                const n = count(p.id);
                return (
                  <article key={p.id} className="flex flex-col rounded-xl border border-line bg-panel p-4 shadow-sm transition hover:border-accent/50">
                    <div className="flex items-start gap-3">
                      <span className="grid size-10 shrink-0 place-items-center rounded-lg bg-accent-soft text-accent">
                        <Icon className="size-5" />
                      </span>
                      <div className="min-w-0 flex-1">
                        <h3 className="truncate font-semibold">
                          <Link to="/app-store/$packageId" params={{ packageId: p.id }} className="hover:text-accent">
                            {p.name}
                          </Link>
                        </h3>
                        <div className="mt-0.5 flex flex-wrap gap-1">
                          <Badge>{KIND[p.kind]}</Badge>
                          {n > 0 && <Badge tone="ok">on {n} node{n === 1 ? "" : "s"}</Badge>}
                          {p.os.includes("windows") && <Badge>Windows too</Badge>}
                        </div>
                      </div>
                    </div>
                    <p className="mt-3 flex-1 text-[13px] text-muted">{p.summary}</p>
                    <div className="mt-3 flex flex-wrap gap-1 text-[11px] text-faint">
                      <span>{p.versions.length > 1 ? `Versions ${p.versions.join(", ")}` : `Version ${p.versions[0]}`}</span>
                      {p.ports.length > 0 && <span>· ports {p.ports.join(", ")}</span>}
                      {p.requires.length > 0 && <span>· needs {p.requires.join(", ")}</span>}
                    </div>
                    <div className="mt-3 flex items-center justify-between gap-2">
                      <a href={p.homepage} target="_blank" rel="noreferrer" className="truncate text-xs text-faint hover:text-accent">
                        {p.license}
                      </a>
                      <div className="flex gap-1.5">
                        <Link to="/app-store/$packageId" params={{ packageId: p.id }}>
                          <Button variant="ghost">Details</Button>
                        </Link>
                        <Button variant="primary" onClick={() => setChosen(p)}>
                          Install
                        </Button>
                      </div>
                    </div>
                  </article>
                );
              })}
            </div>
          )}
        </div>
      </div>

      {(catalog.data?.roadmap.length ?? 0) > 0 && cat === "all" && !q && (
        <section className="mt-8">
          <h2 className="mb-3 flex items-center gap-2 text-sm font-semibold">
            <Sparkles className="size-4 text-accent" /> Coming soon
          </h2>
          <div className="grid gap-2 sm:grid-cols-2 xl:grid-cols-4">
            {catalog.data?.roadmap.map((r) => (
              <div key={r.name} className="rounded-lg border border-dashed border-line p-3">
                <p className="text-[13px] font-medium">{r.name}</p>
                <p className="mt-0.5 text-xs text-muted">{r.description}</p>
              </div>
            ))}
          </div>
        </section>
      )}

      <InstallWizard pkg={chosen} catalog={packages} open={!!chosen} onOpenChange={(o) => !o && setChosen(null)} preselect={search.node} />
    </>
  );
}
