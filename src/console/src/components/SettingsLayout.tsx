import { Link } from "@tanstack/react-router";
import type { LucideIcon } from "lucide-react";
import type { ReactNode } from "react";

export interface SettingsSection {
  id: string;
  label: string;
  icon: LucideIcon;
}

export interface SettingsGroup {
  label: string;
  sections: SettingsSection[];
}

/**
 * Settings shell: a sticky left rail of sections (same look as the sidebar
 * rows: accent bar and soft accent background) and the section's panels on
 * the right. Below `lg` the rail becomes a horizontal scroller.
 */
export function SettingsLayout({ groups, active, children }: { groups: SettingsGroup[]; active: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-4 lg:flex-row lg:items-start">
      <nav aria-label="Settings sections" className="-mx-1 shrink-0 overflow-x-auto lg:sticky lg:top-4 lg:mx-0 lg:w-[200px] lg:overflow-visible">
        <div className="flex gap-3 px-1 lg:flex-col lg:gap-3 lg:px-0">
          {groups.map((g) => (
            <div key={g.label} className="flex shrink-0 items-center gap-1 lg:block">
              <div className="hidden px-2 pb-1 text-[10px] font-semibold tracking-widest text-faint uppercase lg:block">{g.label}</div>
              <ul className="flex gap-1 lg:block lg:space-y-0.5">
                {g.sections.map((s) => {
                  const on = s.id === active;
                  const Icon = s.icon;
                  return (
                    <li key={s.id} className="relative shrink-0">
                      {on && <span className="absolute top-1.5 bottom-1.5 left-0 hidden w-0.5 rounded bg-accent lg:block" />}
                      <Link
                        to="/profile/$section"
                        params={{ section: s.id }}
                        aria-current={on ? "page" : undefined}
                        className={`flex min-w-0 items-center gap-2 rounded-md px-2 py-1.5 text-[13px] whitespace-nowrap transition lg:ml-1.5 ${
                          on ? "bg-accent-soft font-medium text-accent" : "text-muted hover:bg-panel-2 hover:text-fg"
                        }`}
                      >
                        <Icon className="size-3.5 shrink-0" />
                        <span className="truncate">{s.label}</span>
                      </Link>
                    </li>
                  );
                })}
              </ul>
            </div>
          ))}
        </div>
      </nav>
      <div className="min-w-0 flex-1 space-y-4">{children}</div>
    </div>
  );
}
