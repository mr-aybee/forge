import { Link, useRouterState } from "@tanstack/react-router";

const TABS = [
  { to: "/settings", label: "Server" },
  { to: "/settings/analytics", label: "Analytics" },
] as const;

/** Tabs between the server-level settings pages under Administration → Settings. */
export function SettingsTabs() {
  const path = useRouterState({ select: (s) => s.location.pathname });
  return (
    <nav aria-label="Settings sections" className="mb-4 flex gap-1 border-b border-line text-[13px]">
      {TABS.map((t) => {
        const active = path === t.to;
        return (
          <Link
            key={t.to}
            to={t.to}
            aria-current={active ? "page" : undefined}
            className={`-mb-px border-b-2 px-3 py-1.5 focus-visible:outline-2 focus-visible:outline-accent ${active ? "border-accent font-medium text-fg" : "border-transparent text-muted hover:text-fg"}`}
          >
            {t.label}
          </Link>
        );
      })}
    </nav>
  );
}
