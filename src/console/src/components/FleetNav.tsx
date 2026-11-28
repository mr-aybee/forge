import * as DropdownMenu from "@radix-ui/react-dropdown-menu";
import { Link } from "@tanstack/react-router";
import { ChevronRight } from "lucide-react";
import { type KeyboardEvent, useEffect, useMemo, useRef, useState } from "react";

import { useMyCapabilities } from "../api/access";
import { PinnedFlyout, PinnedNav } from "./Pins";
import type { NavBadge, NavBadges, NavTone } from "../api/navBadges";
import { ancestorsOf, isGroup, type NavEntry, type NavGroup, type NavItem, type NavTag, visibleSections } from "../app/nav";

const OPEN_KEY = "forge.nav.open";

function loadOpen(): Set<string> {
  try {
    const v: unknown = JSON.parse(localStorage.getItem(OPEN_KEY) ?? "[]");
    return new Set(Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : []);
  } catch {
    return new Set();
  }
}

function saveOpen(s: Set<string>) {
  try {
    localStorage.setItem(OPEN_KEY, JSON.stringify([...s]));
  } catch {
    /* not remembered */
  }
}

const pillClass: Record<NavTone, string> = {
  ok: "bg-ok-soft text-ok",
  warn: "bg-warn-soft text-warn",
  bad: "bg-bad-soft text-bad font-semibold",
  accent: "bg-accent-soft text-accent",
  muted: "text-faint",
};
const dotClass: Record<NavTone, string> = { ok: "bg-ok", warn: "bg-warn", bad: "bg-bad", accent: "bg-accent", muted: "bg-faint" };

/** Counter next to an item: `n` or `n/total`, with an optional health bullet. */
function Count({ c }: { c?: NavBadge }) {
  if (!c || (c.n <= 0 && !c.total)) return null;
  return (
    <span title={c.title} className={`flex shrink-0 items-center gap-1 rounded px-1.5 font-mono text-[10px] tabular-nums ${pillClass[c.tone]}`}>
      {(c.dot || c.live) && <span className={`size-1.5 rounded-full ${dotClass[c.dot ?? "accent"]} ${c.live ? "animate-pulse" : ""}`} aria-hidden />}
      {c.total ? `${c.n}/${c.total}` : c.n}
    </span>
  );
}

function Tag({ t }: { t?: NavTag }) {
  if (!t) return null;
  return <span className="shrink-0 rounded border border-accent/40 px-1 text-[9px] leading-[14px] font-medium tracking-wide text-accent uppercase">{t}</span>;
}

/** Dot on a collapsed icon: the item's health or counter tone. */
function IconDot({ c }: { c?: NavBadge }) {
  if (!c) return null;
  const tone: NavTone = c.dot ?? (c.tone === "muted" ? "accent" : c.tone);
  if (c.tone === "muted" && !c.dot) return null;
  return <span className={`absolute top-1 right-2 size-1.5 rounded-full ring-2 ring-sidebar ${dotClass[tone]}`} aria-hidden />;
}

const rank: Record<NavTone, number> = { muted: 0, ok: 1, accent: 2, warn: 3, bad: 4 };

/** Sum of the actionable counters under a group (shown on the header while it is closed). Inventory counts are left out. */
function groupCount(g: NavGroup, badges: NavBadges): NavBadge | undefined {
  let n = 0;
  let tone: NavTone = "accent";
  const walk = (es: NavEntry[]) => {
    for (const e of es) {
      if (isGroup(e)) walk(e.children);
      else {
        const c = e.badge ? badges[e.badge] : undefined;
        if (!c || c.tone === "muted" || c.total) continue;
        n += c.n;
        if (rank[c.tone] > rank[tone]) tone = c.tone;
      }
    }
  };
  walk(g.children);
  return n ? { n, tone, title: `${n} needing attention` } : undefined;
}

const badgeOf = (it: NavItem, badges: NavBadges) => (it.badge ? badges[it.badge] : undefined);
const tipOf = (it: NavItem, c?: NavBadge) => (c ? `${it.label} · ${c.title}` : it.label);

const containsActive = (g: NavGroup, isActive: (to: string) => boolean): boolean =>
  g.children.some((c) => (isGroup(c) ? containsActive(c, isActive) : isActive(c.to)));

/**
 * The fleet navigation: headed sections of links and collapsible groups.
 * The expanded state is remembered per browser; the group holding the
 * current page opens by itself. In the collapsed sidebar, groups open a flyout.
 */
export function FleetNav({ collapsed, isActive, badges }: { collapsed: boolean; isActive: (to: string) => boolean; badges: NavBadges }) {
  // Gated items appear once the capabilities are known (no flash of forbidden entries).
  const caps = useMyCapabilities().data;
  const sections = useMemo(() => visibleSections(caps), [caps]);
  const tree = useMemo(() => sections.flatMap((x) => x.entries), [sections]);
  const [open, setOpen] = useState(loadOpen);
  const navRef = useRef<HTMLElement>(null);

  // Open the groups holding the current page (the user can still close them).
  const activeAncestors = ancestorsOf(isActive, tree).join("|");
  useEffect(() => {
    if (!activeAncestors) return;
    setOpen((s) => {
      const ids = activeAncestors.split("|");
      if (ids.every((id) => s.has(id))) return s;
      const n = new Set(s);
      for (const id of ids) n.add(id);
      saveOpen(n);
      return n;
    });
  }, [activeAncestors]);

  const setGroup = (id: string, value: boolean) =>
    setOpen((s) => {
      if (s.has(id) === value) return s;
      const n = new Set(s);
      if (value) n.add(id);
      else n.delete(id);
      saveOpen(n);
      return n;
    });

  // Tree keyboard navigation: ↑/↓ move, → opens or enters, ← closes or goes to the parent, Home/End.
  const onKeyDown = (e: KeyboardEvent<HTMLElement>) => {
    const nav = navRef.current;
    const el = e.target as HTMLElement;
    if (!nav || !el.dataset.navkey) return;
    const all = [...nav.querySelectorAll<HTMLElement>("[data-navkey]")];
    const i = all.indexOf(el);
    const focus = (n: number) => all[Math.max(0, Math.min(all.length - 1, n))]?.focus();
    const gid = el.dataset.group;
    switch (e.key) {
      case "ArrowDown":
        focus(i + 1);
        break;
      case "ArrowUp":
        focus(i - 1);
        break;
      case "Home":
        focus(0);
        break;
      case "End":
        focus(all.length - 1);
        break;
      case "ArrowRight":
        if (!gid) return;
        if (open.has(gid)) focus(i + 1);
        else setGroup(gid, true);
        break;
      case "ArrowLeft":
        if (gid && open.has(gid)) setGroup(gid, false);
        else if (el.dataset.parent) nav.querySelector<HTMLElement>(`[data-group="${el.dataset.parent}"]`)?.focus();
        else return;
        break;
      default:
        return;
    }
    e.preventDefault();
  };

  if (collapsed) {
    return (
      <nav className="nav-scroll mt-3 flex-1 overflow-y-auto px-3 pb-4" aria-label="Main">
        <PinnedFlyout isActive={isActive} />
        {sections.map((sec, i) => (
          <div key={sec.id} role="group" aria-label={sec.heading} className="space-y-0.5">
            {i > 0 && <div className="mx-1 my-2 border-t border-line" aria-hidden />}
            {sec.entries.map((e) =>
              isGroup(e) ? (
                <Flyout key={e.id} g={e} isActive={isActive} badges={badges} />
              ) : (
                <ItemRow key={e.to} it={e} depth={0} collapsed active={isActive(e.to)} badge={badgeOf(e, badges)} />
              ),
            )}
          </div>
        ))}
      </nav>
    );
  }

  const render = (entries: NavEntry[], depth: number, parent?: string) =>
    entries.map((e) => {
      if (!isGroup(e)) return <ItemRow key={e.to} it={e} depth={depth} collapsed={false} active={isActive(e.to)} badge={badgeOf(e, badges)} parent={parent} />;
      const isOpen = open.has(e.id);
      const Icon = e.icon;
      const hasActive = containsActive(e, isActive);
      return (
        <div key={e.id} role="none">
          <button
            type="button"
            data-navkey
            data-group={e.id}
            data-parent={parent}
            aria-expanded={isOpen}
            aria-controls={`nav-${e.id}`}
            onClick={() => setGroup(e.id, !isOpen)}
            style={{ paddingLeft: 8 + depth * 14 }}
            className={`flex w-full min-w-0 items-center gap-2.5 rounded-md py-1.5 pr-2 text-left transition hover:bg-panel-2 hover:text-fg ${hasActive && !isOpen ? "text-accent" : "text-muted"}`}
          >
            <Icon className={`shrink-0 ${parent ? "size-3.5" : "size-4"}`} />
            <span className={`min-w-0 flex-1 truncate ${parent ? "text-[12.5px]" : ""}`}>{e.label}</span>
            {!isOpen && <Count c={groupCount(e, badges)} />}
            <ChevronRight className={`size-3.5 shrink-0 text-faint transition ${isOpen ? "rotate-90" : ""}`} />
          </button>
          {isOpen && (
            <div id={`nav-${e.id}`} role="group" aria-label={e.label} className="relative ml-[15px] space-y-0.5 border-l border-line/70 pl-1">
              {render(e.children, 0, e.id)}
            </div>
          )}
        </div>
      );
    });

  return (
    <nav ref={navRef} onKeyDown={onKeyDown} className="nav-scroll mt-3 flex-1 overflow-y-auto px-3 pb-4" aria-label="Main">
      <PinnedNav open={open} setGroup={setGroup} isActive={isActive} />
      {sections.map((sec) => (
        <div key={sec.id} role="group" aria-label={sec.heading} className="space-y-0.5 [&:not(:first-child)]:mt-3">
          {sec.heading && <div className="px-2 pt-1 pb-1 text-[10px] font-semibold tracking-widest text-faint uppercase select-none">{sec.heading}</div>}
          {render(sec.entries, 0)}
        </div>
      ))}
    </nav>
  );
}

function ItemRow({ it, depth, collapsed, active, badge, parent }: { it: NavItem; depth: number; collapsed: boolean; active: boolean; badge?: NavBadge; parent?: string }) {
  const Icon = it.icon;
  const nested = !!parent;
  return (
    <div className="relative flex items-center">
      {active && <span className={`absolute top-1.5 bottom-1.5 w-0.5 rounded bg-accent ${nested ? "-left-[6px]" : "-left-2"}`} />}
      <Link
        to={it.to}
        data-navkey
        data-parent={parent}
        title={collapsed ? tipOf(it, badge) : undefined}
        aria-label={collapsed ? tipOf(it, badge) : undefined}
        aria-current={active ? "page" : undefined}
        style={{ paddingLeft: collapsed ? undefined : 8 + depth * 14 }}
        className={`relative flex min-w-0 flex-1 items-center gap-2.5 rounded-md py-1.5 pr-2 transition ${collapsed ? "justify-center px-0" : ""} ${
          active ? "bg-accent-soft font-medium text-accent" : "text-muted hover:bg-panel-2 hover:text-fg"
        }`}
      >
        <Icon className={`shrink-0 ${nested ? "size-3.5" : "size-4"}`} />
        {!collapsed && <span className={`min-w-0 flex-1 truncate ${nested ? "text-[12.5px]" : ""}`}>{it.label}</span>}
        {!collapsed && <Tag t={it.tag} />}
        {!collapsed && <Count c={badge} />}
        {collapsed && <IconDot c={badge} />}
      </Link>
    </div>
  );
}

/* --------------------------------------------- collapsed sidebar flyouts */

const menuContent = "animate-in z-50 min-w-[200px] rounded-lg border border-line bg-panel p-1 text-[13px] shadow-[var(--shadow-pop)]";
const menuItem = "flex cursor-pointer items-center gap-2 rounded-md px-2 py-1.5 text-muted outline-none data-[highlighted]:bg-panel-2 data-[highlighted]:text-fg";

function Flyout({ g, isActive, badges }: { g: NavGroup; isActive: (to: string) => boolean; badges: NavBadges }) {
  const Icon = g.icon;
  const active = containsActive(g, isActive);
  const c = groupCount(g, badges);
  return (
    <DropdownMenu.Root modal={false}>
      <DropdownMenu.Trigger asChild>
        <button
          type="button"
          title={c ? `${g.label} · ${c.title}` : g.label}
          aria-label={c ? `${g.label} · ${c.title}` : g.label}
          className={`relative flex w-full items-center justify-center rounded-md py-1.5 transition data-[state=open]:bg-panel-2 ${active ? "bg-accent-soft text-accent" : "text-muted hover:bg-panel-2 hover:text-fg"}`}
        >
          {active && <span className="absolute top-1.5 bottom-1.5 -left-2 w-0.5 rounded bg-accent" />}
          <Icon className="size-4" />
          <IconDot c={c} />
        </button>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content side="right" align="start" sideOffset={8} className={menuContent}>
          <DropdownMenu.Label className="px-2 pt-1 pb-1.5 text-[10px] font-semibold tracking-widest text-faint uppercase">{g.label}</DropdownMenu.Label>
          <FlyoutEntries entries={g.children} isActive={isActive} badges={badges} />
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}

function FlyoutEntries({ entries, isActive, badges }: { entries: NavEntry[]; isActive: (to: string) => boolean; badges: NavBadges }) {
  return (
    <>
      {entries.map((e) => {
        const Icon = e.icon;
        if (isGroup(e)) {
          return (
            <DropdownMenu.Sub key={e.id}>
              <DropdownMenu.SubTrigger className={`${menuItem} data-[state=open]:bg-panel-2`}>
                <Icon className="size-3.5 shrink-0" />
                <span className="min-w-0 flex-1 truncate">{e.label}</span>
                <ChevronRight className="size-3.5 shrink-0 text-faint" />
              </DropdownMenu.SubTrigger>
              <DropdownMenu.Portal>
                <DropdownMenu.SubContent sideOffset={6} className={menuContent}>
                  <FlyoutEntries entries={e.children} isActive={isActive} badges={badges} />
                </DropdownMenu.SubContent>
              </DropdownMenu.Portal>
            </DropdownMenu.Sub>
          );
        }
        const active = isActive(e.to);
        return (
          <DropdownMenu.Item key={e.to} asChild>
            <Link to={e.to} aria-current={active ? "page" : undefined} className={`${menuItem} ${active ? "font-medium text-accent" : ""}`}>
              <Icon className="size-3.5 shrink-0" />
              <span className="min-w-0 flex-1 truncate">{e.label}</span>
              <Tag t={e.tag} />
              <Count c={badgeOf(e, badges)} />
            </Link>
          </DropdownMenu.Item>
        );
      })}
    </>
  );
}
