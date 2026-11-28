import { useRouterState } from "@tanstack/react-router";
import { PanelLeftClose, PanelLeftOpen, Search } from "lucide-react";

import { useNavBadges } from "../api/navBadges";
import { FleetNav } from "./FleetNav";
import { usePrefs } from "../app/prefs";

/** Sidebar width; Layout offsets the page by the same amount. */
export const SIDEBAR_WIDTH = { open: 216, collapsed: 56 } as const;

/** Fleet navigation: search, the nav tree and the collapse toggle. */
export function Sidebar({ onSearch, mobileOpen = false }: { onSearch: () => void; mobileOpen?: boolean }) {
  const { sidebarCollapsed: collapsed, toggleSidebar } = usePrefs();
  const path = useRouterState({ select: (s) => s.location.pathname });
  return (
    <aside
      className={`sidebar-ocean fixed inset-y-0 left-0 z-30 flex-col border-r transition-[width] ${mobileOpen ? "flex" : "hidden md:flex"}`}
      style={{ width: collapsed ? SIDEBAR_WIDTH.collapsed : SIDEBAR_WIDTH.open }}
    >
      <FleetPanel collapsed={collapsed} path={path} onSearch={onSearch} />
      <button
        onClick={toggleSidebar}
        className="flex items-center gap-2 border-t border-line px-4 py-3 text-muted hover:text-fg"
        aria-label={collapsed ? "Expand sidebar" : "Collapse sidebar"}
      >
        {collapsed ? <PanelLeftOpen className="size-4" /> : <PanelLeftClose className="size-4" />}
        {!collapsed && <span className="text-xs">Collapse</span>}
      </button>
    </aside>
  );
}

function FleetPanel({ collapsed, path, onSearch }: { collapsed: boolean; path: string; onSearch: () => void }) {
  const isActive = (to: string) => (to === "/" || to === "/clusters" ? path === to : path === to || path.startsWith(`${to}/`));
  const badges = useNavBadges();
  return (
    <>
      <div className="flex h-[58px] items-center px-4">
        {collapsed ? (
          <span className="mx-auto text-[10px] font-semibold tracking-widest text-faint">FLEET</span>
        ) : (
          <div className="leading-tight">
            <div className="font-semibold tracking-tight">Orbyn Forge</div>
            <div className="text-[10px] tracking-widest text-faint uppercase">Fleet</div>
          </div>
        )}
      </div>
      <div className="px-3">
        <button onClick={onSearch} className="flex w-full items-center justify-center gap-2 rounded-md border border-line px-2.5 py-1.5 text-muted hover:border-accent/50">
          <Search className="size-4" />
          {!collapsed && (
            <>
              <span className="flex-1 text-left">Search</span>
              <kbd className="rounded border border-line px-1 text-[10px]">Ctrl K</kbd>
            </>
          )}
        </button>
      </div>
      <FleetNav collapsed={collapsed} isActive={isActive} badges={badges} />
    </>
  );
}
