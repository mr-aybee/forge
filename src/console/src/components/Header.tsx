import * as Menu from "@radix-ui/react-dropdown-menu";
import { Link, useRouterState } from "@tanstack/react-router";
import { LogOut, Menu as MenuIcon, Moon, Settings, ShieldCheck, ShieldAlert, Sun, Workflow } from "lucide-react";
import { useQueryClient } from "@tanstack/react-query";

import { post } from "../api/client";
import { useJobs, useMe } from "../api/hooks";
import { loginOf } from "../api/users";
import { ALL_ITEMS } from "../app/nav";
import { usePrefs } from "../app/prefs";
import { IconButton } from "./ui";
import { Notifications } from "./Notifications";
import { UpdateBadge } from "./UpdateBadge";

export function Header({ onJobs, onMobileNav }: { onJobs: () => void; onMobileNav: () => void }) {
  const { theme, toggleTheme } = usePrefs();
  const me = useMe();
  const qc = useQueryClient();
  const path = useRouterState({ select: (s) => s.location.pathname });
  const jobs = useJobs();
  const page = ALL_ITEMS.find((i) => (i.to === "/" ? path === "/" : path.startsWith(i.to)))?.label ?? "";

  const running = (jobs.data ?? []).filter((j) => j.status === "running" || j.status === "queued").length;
  const signOut = async () => {
    await post("/api/v1/auth/logout");
    qc.clear();
    location.assign("/");
  };

  const item = "flex cursor-pointer items-center gap-2 rounded-md px-2 py-1.5 outline-none data-[highlighted]:bg-panel-2";
  const content = "z-50 min-w-64 rounded-lg border border-line bg-panel p-1 shadow-[var(--shadow-pop)]";

  return (
    <header className="sticky top-0 z-20 flex h-[58px] items-center gap-3 border-b border-line bg-panel/85 px-4 backdrop-blur md:px-6">
      <IconButton label="Open navigation" className="md:hidden" onClick={onMobileNav}>
        <MenuIcon className="size-5" />
      </IconButton>
      <nav aria-label="Breadcrumb" className="flex min-w-0 items-center gap-2 text-muted">
        <span className="truncate">Orbyn Forge</span>
        <span className="text-faint">/</span>
        <span className="truncate font-medium text-fg">{page}</span>
      </nav>
      <div className="ml-auto flex items-center gap-1">
        <UpdateBadge />
        <button
          onClick={onJobs}
          className="flex items-center gap-1.5 rounded-md px-2 py-1.5 text-muted hover:bg-panel-2 hover:text-fg"
          aria-label={`Jobs (${running} active)`}
        >
          <Workflow className="size-4" />
          <span className="hidden text-xs sm:inline">Jobs</span>
          {running > 0 && <span className="rounded-full bg-accent px-1.5 text-[10px] font-semibold text-bg">{running}</span>}
        </button>

        <Notifications />

        <IconButton label={theme === "light" ? "Switch to dark mode" : "Switch to light mode"} onClick={toggleTheme}>
          {theme === "light" ? <Moon className="size-4" /> : <Sun className="size-4" />}
        </IconButton>

        <Menu.Root>
          <Menu.Trigger className="ml-1 flex items-center gap-2 rounded-md p-1 hover:bg-panel-2" aria-label="Account">
            <span className="grid size-7 place-items-center rounded-full bg-accent-soft text-xs font-semibold text-accent">
              {(me.data?.user.display_name ?? "?").slice(0, 1).toUpperCase()}
            </span>
          </Menu.Trigger>
          <Menu.Portal>
            <Menu.Content align="end" sideOffset={6} className={content}>
              <div className="px-2 py-2">
                <div className="font-medium">{me.data?.user.display_name}</div>
                <div className="text-xs text-muted">{me.data && loginOf(me.data.user)}</div>
                <div className={`mt-1.5 flex items-center gap-1 text-xs ${me.data?.mfa ? "text-ok" : "text-warn"}`}>
                  {me.data?.mfa ? <ShieldCheck className="size-3.5" /> : <ShieldAlert className="size-3.5" />}
                  {me.data?.mfa ? "MFA-verified session" : "Not MFA-verified — sensitive actions are blocked"}
                </div>
              </div>
              <Menu.Separator className="my-1 h-px bg-line" />
              <Menu.Item asChild className={item}>
                <Link to="/profile">
                  <Settings className="size-4" /> Profile & security
                </Link>
              </Menu.Item>
              <Menu.Item onSelect={toggleTheme} className={item}>
                {theme === "light" ? <Moon className="size-4" /> : <Sun className="size-4" />}
                {theme === "light" ? "Dark mode" : "Light mode"}
              </Menu.Item>
              <Menu.Item onSelect={() => void signOut()} className={`${item} text-bad`}>
                <LogOut className="size-4" /> Sign out
              </Menu.Item>
            </Menu.Content>
          </Menu.Portal>
        </Menu.Root>
      </div>
    </header>
  );
}
