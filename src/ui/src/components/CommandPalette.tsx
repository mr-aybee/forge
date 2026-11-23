import * as Dialog from "@radix-ui/react-dialog";
import { useNavigate } from "@tanstack/react-router";
import {
  ArrowDown,
  AppWindow,
  ArrowUp,
  Boxes,
  Loader2,
  Clock,
  CornerDownLeft,
  ListChecks,
  type LucideIcon,
  Moon,
  Plus,
  Search,
  Server,
  SquareTerminal,
  Zap,
} from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";

import { useAllEnvironments, useProjects, useServices, useClusters, useLocalNames, useServers } from "../api/hooks";
import { nodeName } from "../api/types";
import { useMyCapabilities } from "../api/access";
import { allowed, flatItems } from "../app/nav";
import { pinTarget, usePins } from "../api/pins";
import { PIN_ICON } from "./Pins";
import { usePrefs } from "../app/prefs";
import { useTerminals } from "../app/terminals";

interface Item {
  id: string;
  group: string;
  label: string;
  hint?: string;
  icon: LucideIcon;
  /** Extra words matched by search (not shown). */
  keywords?: string;
  run: () => void;
  /** Secondary action (Ctrl+Enter or the trailing button). */
  alt?: { label: string; icon: LucideIcon; run: () => void };
  dot?: string;
}

const RECENT_KEY = "forge.palette.recent";
const loadRecent = (): string[] => {
  try {
    return JSON.parse(localStorage.getItem(RECENT_KEY) ?? "[]") as string[];
  } catch {
    return [];
  }
};

const statusDot: Record<string, string> = { online: "bg-ok", offline: "bg-warn", pending: "bg-faint", revoked: "bg-bad" };

/** Groups whose entries come from fleet data (not shown until the user types). */
const DATA_GROUPS = new Set(["Nodes", "Services", "Projects", "Clusters"]);
const PER_GROUP = 8;

/** Dot for a service from its instances: any failed → bad, any deployed → ok, in progress → warn. */
function serviceDot(statuses: string[]): string {
  if (!statuses.length) return "bg-faint";
  if (statuses.some((s) => /fail|error/.test(s))) return "bg-bad";
  if (statuses.some((s) => s.endsWith("ing"))) return "bg-warn";
  if (statuses.some((s) => s === "deployed" || s === "running")) return "bg-ok";
  return "bg-faint";
}

/** Scores `text` against `needle`: prefix > word start > substring > subsequence. */
function score(text: string, needle: string): number {
  const t = text.toLowerCase();
  if (!needle) return 1;
  if (t.startsWith(needle)) return 100 - t.length / 100;
  const i = t.indexOf(needle);
  if (i > 0 && /[\s/·@-]/.test(t[i - 1] ?? "")) return 80 - i / 100;
  if (i >= 0) return 60 - i / 100;
  let j = 0;
  for (const c of t) if (c === needle[j]) j++;
  return j === needle.length ? 20 : 0;
}

/** Ctrl/Cmd+K: jump to pages, nodes and clusters, or run quick actions. */
export function CommandPalette({ open, onOpenChange, onJobs }: { open: boolean; onOpenChange: (o: boolean) => void; onJobs: () => void }) {
  const navigate = useNavigate();
  const terminals = useTerminals();
  const { toggleTheme } = usePrefs();
  const servers = useServers();
  const clusters = useClusters();
  // Loaded only while the palette is open; cached by TanStack Query between openings.
  const apps = useServices(undefined, undefined, open);
  const environments = useAllEnvironments();
  const caps = useMyCapabilities().data;
  const localNames = useLocalNames(open);
  const [q, setQ] = useState("");
  const [active, setActive] = useState(0);
  const [recent, setRecent] = useState(loadRecent);
  const list = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        onOpenChange(!open);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onOpenChange]);

  useEffect(() => {
    if (open) {
      setQ("");
      setActive(0);
    }
  }, [open]);

  const serverList = useMemo(() => (Array.isArray(servers.data) ? servers.data : []), [servers.data]);
  const clusterList = useMemo(() => (Array.isArray(clusters.data) ? clusters.data : []), [clusters.data]);
  const appList = useMemo(() => (Array.isArray(apps.data) ? apps.data : []), [apps.data]);
  const nsById = useMemo(() => new Map((Array.isArray(environments.data) ? environments.data : []).map((n) => [n.id, n])), [environments.data]);
  const projects = useProjects();
  const pinsData = usePins().data;
  const groupById = useMemo(() => new Map((Array.isArray(projects.data) ? projects.data : []).map((g) => [g.id, g.name])), [projects.data]);
  // Local DNS names (e.g. lab-b.lab.local) by server id and by service (id or name).
  const [nodeDns, svcDns] = useMemo(() => {
    const byNode = new Map<string, string[]>();
    const bySvc = new Map<string, string[]>();
    const list = localNames.data && Array.isArray(localNames.data.names) ? localNames.data.names : [];
    for (const n of list) {
      if (!n?.name) continue;
      const m = n.kind === "service" && n.app ? bySvc : n.kind === "node" ? byNode : null;
      const key = n.kind === "service" ? n.app : n.server_id;
      if (!m || !key) continue;
      m.set(key, [...(m.get(key) ?? []), n.name]);
    }
    return [byNode, bySvc] as const;
  }, [localNames.data]);

  const all = useMemo<Item[]>(() => {
    const go = (to: string) => () => void navigate({ to });
    return [
      { id: "add-node", group: "Actions", label: "Add a node", icon: Plus, keywords: "onboard enroll server", run: go("/nodes/add") },
      ...(caps?.terminal ? [{ id: "new-session", group: "Actions", label: "New terminal session", icon: SquareTerminal, keywords: "ssh shell", run: go("/terminal") }] : []),
      ...(caps?.package_install ? [{ id: "install", group: "Actions", label: "Install a package", icon: Zap, keywords: "app store database", run: go("/app-store") }] : []),
      { id: "jobs", group: "Actions", label: "Show recent jobs", icon: ListChecks, run: onJobs },
      { id: "theme", group: "Actions", label: "Toggle dark mode", icon: Moon, keywords: "theme light", run: toggleTheme },
      ...(pinsData?.items ?? []).map((p) => {
        const t = pinTarget(p);
        return { id: `pin:${p.kind}:${p.id}`, group: "Pinned", label: p.label, hint: [p.kind, p.sublabel].filter(Boolean).join(" · "), keywords: p.id, icon: PIN_ICON[p.kind], run: () => void navigate(t) };
      }),
      ...flatItems().filter((i) => allowed(i, caps)).map((i) => ({ id: `nav:${i.to}`, group: "Pages", label: i.label, hint: i.trail.join(" › ") || undefined, icon: i.icon, run: go(i.to) })),
      ...clusterList.map((c) => {
        const total = c.node_count ?? 0;
        const online = c.nodes_online ?? 0;
        return {
          id: `cl:${c.id}`,
          group: "Clusters",
          label: c.name,
          hint: `${online}/${total} nodes online${c.description ? ` · ${c.description}` : ""}`,
          keywords: c.id,
          icon: Boxes,
          dot: !total ? "bg-faint" : online === total ? "bg-ok" : online > 0 ? "bg-warn" : "bg-bad",
          run: go(`/clusters/${c.id}`),
        };
      }),
      ...serverList.map((s) => {
        const dns = [s.dns?.name, ...(nodeDns.get(s.id) ?? [])].filter(Boolean);
        return {
          id: `n:${s.id}`,
          group: "Nodes",
          label: nodeName(s),
          hint: [s.role === "control" ? "Control plane" : "Worker", s.status, dns[0], `${s.os}/${s.arch}`].filter(Boolean).join(" · "),
          keywords: `${s.hostname} ${s.role} ${s.id} ${dns.join(" ")}`,
          icon: Server,
          dot: statusDot[s.status],
          run: go(`/nodes/${s.id}`),
          alt: s.status === "revoked" ? undefined : { label: "Terminal", icon: SquareTerminal, run: () => terminals.open(s) },
        };
      }),
      ...(Array.isArray(projects.data) ? projects.data : []).map((g) => ({
        id: `project:${g.id}`,
        group: "Projects",
        label: g.name,
        hint: [`${g.environments.length} environment${g.environments.length === 1 ? "" : "s"}`, `${g.service_count} service${g.service_count === 1 ? "" : "s"}`, g.description].filter(Boolean).join(" · "),
        keywords: `${g.id} ${g.environments.map((e) => e.name).join(" ")}`,
        icon: AppWindow,
        run: () => void navigate({ to: "/projects", search: { sel: `project:${g.id}` } }),
      })),
      ...appList.map((a) => {
        const ns = nsById.get(a.environment_id);
        const cl = ns?.cluster_id ? clusterList.find((c) => c.id === ns.cluster_id) : undefined;
        const gname = ns?.project_id ? groupById.get(ns.project_id) : undefined;
        const inst = Array.isArray(a.instances) ? a.instances : [];
        const nodes = inst.map((i) => serverList.find((s) => s.id === i.server_id)).filter((s): s is NonNullable<typeof s> => !!s).map(nodeName);
        const dns = [...(svcDns.get(a.id) ?? []), ...(svcDns.get(a.name) ?? [])];
        return {
          id: `app:${a.id}`,
          group: "Services",
          label: a.name,
          hint: [[gname, ns?.name, cl?.name].filter(Boolean).join(" › "), a.runtime, dns[0], nodes.length ? `on ${nodes.join(", ")}` : "not placed"].filter(Boolean).join(" · "),
          keywords: `${a.id} ${dns.join(" ")} ${nodes.join(" ")} ${ns?.name ?? ""} ${gname ?? ""}`,
          icon: Boxes,
          dot: serviceDot(inst.map((i) => i.status ?? "")),
          run: go(`/services/${a.id}`),
        };
      }),
    ];
  }, [serverList, clusterList, appList, nsById, groupById, projects.data, pinsData, nodeDns, svcDns, navigate, onJobs, toggleTheme, terminals, caps]);

  const items = useMemo(() => {
    const needle = q.trim().toLowerCase();
    if (!needle) {
      const byId = new Map(all.map((i) => [i.id, i]));
      const rec = recent.map((id) => byId.get(id)).filter((i): i is Item => !!i).map((i) => ({ ...i, group: "Recent" }));
      const seen = new Set(rec.map((i) => i.id));
      return [...rec, ...all.filter((i) => !seen.has(i.id) && !DATA_GROUPS.has(i.group)), ...all.filter((i) => i.group === "Nodes" && !seen.has(i.id)).slice(0, 6)];
    }
    // Rank, then keep the groups together in rank order of their best hit.
    const ranked = all
      .map((i) => ({ i, s: Math.max(score(i.label, needle), score(`${i.hint ?? ""} ${i.keywords ?? ""}`, needle) * 0.6) }))
      .filter((x) => x.s > 0)
      .sort((a, b) => b.s - a.s);
    const order: string[] = [];
    for (const r of ranked) if (!order.includes(r.i.group)) order.push(r.i.group);
    return order.flatMap((g) => ranked.filter((r) => r.i.group === g).slice(0, PER_GROUP).map((r) => r.i));
  }, [q, all, recent]);

  useEffect(() => {
    list.current?.querySelector(`[data-index="${active}"]`)?.scrollIntoView({ block: "nearest" });
  }, [active]);

  const choose = (i: Item | undefined, alt = false) => {
    if (!i) return;
    const next = [i.id, ...recent.filter((x) => x !== i.id)].slice(0, 5);
    setRecent(next);
    try {
      localStorage.setItem(RECENT_KEY, JSON.stringify(next));
    } catch {
      /* not remembered */
    }
    onOpenChange(false);
    (alt && i.alt ? i.alt.run : i.run)();
  };

  let lastGroup = "";
  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-40 bg-black/40 backdrop-blur-[2px]" />
        <Dialog.Content className="animate-in fixed top-[12vh] left-1/2 z-50 flex max-h-[70vh] w-[min(640px,calc(100vw-32px))] -translate-x-1/2 flex-col overflow-hidden rounded-xl border border-line bg-panel shadow-[var(--shadow-pop)]">
          <Dialog.Title className="sr-only">Command palette</Dialog.Title>
          <Dialog.Description className="sr-only">Search pages, nodes, services, clusters and actions</Dialog.Description>
          <div className="flex items-center gap-3 border-b border-line px-4">
            <Search className="size-[18px] shrink-0 text-faint" />
            <input
              autoFocus
              data-bare className="h-14 w-full bg-transparent text-[15px] outline-none placeholder:text-faint"
              placeholder="Search pages, nodes, services, clusters, local names or IDs…"
              value={q}
              role="combobox"
              aria-expanded
              aria-controls="palette-list"
              aria-activedescendant={items[active] ? `pal-${items[active].id}` : undefined}
              onChange={(e) => {
                setQ(e.target.value);
                setActive(0);
              }}
              onKeyDown={(e) => {
                if (e.key === "ArrowDown") {
                  e.preventDefault();
                  setActive((a) => (a + 1) % Math.max(1, items.length));
                } else if (e.key === "ArrowUp") {
                  e.preventDefault();
                  setActive((a) => (a - 1 + items.length) % Math.max(1, items.length));
                } else if (e.key === "Enter") {
                  e.preventDefault();
                  choose(items[active], e.ctrlKey || e.metaKey);
                }
              }}
            />
            <kbd className="hidden shrink-0 rounded border border-line px-1.5 py-0.5 text-[10px] text-faint sm:block">ESC</kbd>
          </div>

          <div ref={list} id="palette-list" role="listbox" className="min-h-0 flex-1 overflow-y-auto p-2">
            {items.map((i, n) => {
              const header = i.group !== lastGroup ? i.group : null;
              lastGroup = i.group;
              const on = n === active;
              const Icon = i.group === "Recent" ? Clock : i.icon;
              return (
                <div key={`${i.group}:${i.id}`}>
                  {header && <div className="px-2.5 pt-3 pb-1.5 text-[11px] font-medium text-faint first:pt-1">{header}</div>}
                  <div
                    id={`pal-${i.id}`}
                    role="option"
                    data-index={n}
                    aria-selected={on}
                    onMouseMove={() => !on && setActive(n)}
                    onClick={() => choose(i)}
                    className={`group flex cursor-pointer items-center gap-3 rounded-lg px-2.5 py-2 ${on ? "bg-accent-soft" : ""}`}
                  >
                    <span className={`relative grid size-8 shrink-0 place-items-center rounded-md border ${on ? "border-accent/30 bg-panel text-accent" : "border-line bg-panel-2 text-muted"}`}>
                      <Icon className="size-4" />
                      {i.dot && <span className={`absolute -right-0.5 -bottom-0.5 size-2.5 rounded-full border-2 border-panel ${i.dot}`} />}
                    </span>
                    <span className="min-w-0 flex-1">
                      <span className={`block truncate text-[13px] ${on ? "font-medium text-fg" : ""}`}>{i.label}</span>
                      {i.hint && <span className="block truncate text-xs text-faint">{i.hint}</span>}
                    </span>
                    {i.alt && (
                      <button
                        tabIndex={-1}
                        onClick={(e) => {
                          e.stopPropagation();
                          choose(i, true);
                        }}
                        className={`flex items-center gap-1 rounded-md border px-2 py-1 text-xs ${on ? "border-accent/30 bg-panel text-accent" : "invisible border-line text-muted group-hover:visible"}`}
                      >
                        <i.alt.icon className="size-3.5" /> {i.alt.label}
                      </button>
                    )}
                    {on && <CornerDownLeft className="size-3.5 shrink-0 text-accent" />}
                  </div>
                </div>
              );
            })}
            {items.length === 0 && (
              <div className="px-3 py-10 text-center">
                <p className="text-[13px] text-muted">No results for “{q}”</p>
                <p className="mt-1 text-xs text-faint">
                  {apps.isLoading ? (
                    <span className="inline-flex items-center gap-1.5"><Loader2 className="size-3.5 animate-spin" /> Loading services…</span>
                  ) : (
                    "Try a node, service or cluster name, a local name like node-1.lab.local, an ID or a page."
                  )}
                </p>
              </div>
            )}
          </div>

          <div className="flex items-center gap-4 border-t border-line bg-panel-2/50 px-4 py-2 text-[11px] text-faint">
            <span className="flex items-center gap-1">
              <kbd className="rounded border border-line bg-panel px-1"><ArrowUp className="inline size-3" /></kbd>
              <kbd className="rounded border border-line bg-panel px-1"><ArrowDown className="inline size-3" /></kbd>
              navigate
            </span>
            <span className="flex items-center gap-1">
              <kbd className="rounded border border-line bg-panel px-1">↵</kbd> open
            </span>
            <span className="hidden items-center gap-1 sm:flex">
              <kbd className="rounded border border-line bg-panel px-1">Ctrl ↵</kbd> terminal
            </span>
            <span className="ml-auto">{items.length} result{items.length === 1 ? "" : "s"}</span>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
