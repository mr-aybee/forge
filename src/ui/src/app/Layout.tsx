import { Outlet, useRouterState } from "@tanstack/react-router";
import { ScrollText, ShieldCheck, SquareTerminal } from "lucide-react";
import { Suspense, lazy, useCallback, useState } from "react";

import { useLiveEvents, useVersion, useWorkspace } from "../api/hooks";
import type { LiveEvent } from "../api/types";
import { BottomDrawer, eventLine, type StreamLine } from "../components/BottomDrawer";
import { CommandPalette } from "../components/CommandPalette";
import { Header } from "../components/Header";
import { JobsDrawer } from "../components/JobsDrawer";
import { Sidebar } from "../components/Sidebar";
import { usePrefs } from "./prefs";
import { RouteGate } from "./RouteGate";
import { StepUpProvider } from "./stepUp";
import { TerminalsProvider, useTerminals } from "./terminals";

// xterm.js is only loaded once a terminal is opened.
const TerminalDock = lazy(() => import("../components/terminal/TerminalDock"));

/** Root layout: pop-out windows and the connector consent page render bare; everything else gets the shell. */
export function Layout() {
  const path = useRouterState({ select: (s) => s.location.pathname });
  if (path.startsWith("/popout/") || path === "/connect" || path === "/device") return <Outlet />;
  return (
    <StepUpProvider>
      <TerminalsProvider>
        <Shell />
      </TerminalsProvider>
    </StepUpProvider>
  );
}

/** Authenticated shell: sidebar + header + content + terminal dock + footer. */
function Shell() {
  const path = useRouterState({ select: (s) => s.location.pathname });
  const { sidebarCollapsed } = usePrefs();
  const workspace = useWorkspace();
  const version = useVersion();
  const terminals = useTerminals();
  const [palette, setPalette] = useState(false);
  const [jobs, setJobs] = useState(false);
  const [consoleOpen, setConsoleOpen] = useState(false);
  const [dockWanted, setDockWanted] = useState(false);
  const [mobileNav, setMobileNav] = useState(false);
  const [lines, setLines] = useState<StreamLine[]>([]);

  const onEvent = useCallback((ev: LiveEvent) => setLines((l) => [...l.slice(-499), eventLine(ev)]), []);
  useLiveEvents(true, onEvent);
  const openJobs = useCallback(() => setJobs(true), []);
  const showDock = dockWanted || terminals.sessions.length > 0;

  return (
    <div className="flex h-screen overflow-hidden">
      <Sidebar onSearch={() => setPalette(true)} mobileOpen={mobileNav} />
      {mobileNav && (
        <div className="fixed inset-0 z-40 md:hidden" onClick={() => setMobileNav(false)}>
          <div className="absolute inset-0 bg-black/40" />
        </div>
      )}
      <div className={`flex min-w-0 flex-1 flex-col transition-[margin] ${sidebarCollapsed ? "md:ml-[56px]" : "md:ml-[216px]"}`}>
        <Header onJobs={openJobs} onMobileNav={() => setMobileNav(true)} />
        <main className="min-h-0 flex-1 overflow-y-auto px-4 py-5 md:px-6">
          <div className="mx-auto max-w-[1600px]">
            {workspace.isError ? (
              <p className="text-muted">You don't have access to this control plane yet. Ask an administrator to grant you a role.</p>
            ) : (
              <div key={path} className="view-enter">
                <RouteGate path={path}>
                  <Outlet />
                </RouteGate>
              </div>
            )}
          </div>
        </main>
        <BottomDrawer open={consoleOpen} onClose={() => setConsoleOpen(false)} lines={lines} onClear={() => setLines([])} />
        {showDock && (
          <Suspense fallback={<div className="h-9 border-t border-line bg-panel" />}>
            <TerminalDock onHide={() => setDockWanted(false)} />
          </Suspense>
        )}
        <footer className="flex h-8 shrink-0 items-center gap-4 border-t border-line bg-panel px-4 text-[11px] text-faint md:px-6">
          <button
            className="flex items-center gap-1.5 hover:text-fg"
            onClick={() => {
              if (!showDock) {
                setDockWanted(true);
                terminals.setDock({ open: true });
              } else {
                terminals.setDock({ open: !terminals.dock.open });
              }
            }}
          >
            <SquareTerminal className="size-3.5" /> Terminal {terminals.sessions.length > 0 && `(${terminals.sessions.length})`}
          </button>
          <button className="flex items-center gap-1.5 hover:text-fg" onClick={() => setConsoleOpen((o) => !o)}>
            <ScrollText className="size-3.5" /> Events {lines.length > 0 && `(${lines.length})`}
          </button>
          <span className="hidden items-center gap-1 lg:flex">
            <ShieldCheck className="size-3.5 text-ok" /> Every change is recorded in the hash-chained audit log
          </span>
          <button className="ml-auto hover:text-fg" onClick={() => setPalette(true)}>
            <kbd className="rounded border border-line px-1">Ctrl K</kbd> Command
          </button>
          <span title={version.data?.build ? `Build ${version.data.build}` : undefined}>
            Orbyn Forge {version.data?.version}
          </span>
        </footer>
      </div>
      <CommandPalette open={palette} onOpenChange={setPalette} onJobs={openJobs} />
      <JobsDrawer open={jobs} onOpenChange={setJobs} />
    </div>
  );
}
