import { Boxes, Workflow } from "lucide-react";

import type { ClusterMode } from "../api/types";

type ModeInfo = { id: ClusterMode; label: string; icon: typeof Boxes; text: string };
export const DEFAULT_MODE: ModeInfo = { id: "default", label: "Default", icon: Workflow, text: "Forge orchestration. Runs system services and containers." };
export const MODES: ModeInfo[] = [
  DEFAULT_MODE,
  { id: "swarm", label: "Docker Swarm", icon: Boxes, text: "Docker Swarm schedules containers across nodes. Container services only; every node needs Docker Engine." },
];

/** The cluster mode: chosen once at creation, shown read-only afterwards. */
export function ModePicker({ value, locked, onChange }: { value: ClusterMode; locked: boolean; onChange: (m: ClusterMode) => void }) {
  if (locked) {
    const m = MODES.find((x) => x.id === value) ?? DEFAULT_MODE;
    return (
      <p className="flex items-center gap-2 text-[13px]">
        <m.icon className="size-4 text-muted" aria-hidden="true" /> <span className="font-medium">{m.label}</span>
        <span className="text-muted">· set when the cluster was created</span>
      </p>
    );
  }
  return (
    <div role="radiogroup" aria-label="Cluster mode" className="grid gap-2 sm:grid-cols-2">
      {MODES.map((m) => {
        const on = m.id === value;
        return (
          <button
            key={m.id}
            type="button"
            role="radio"
            aria-checked={on}
            onClick={() => onChange(m.id)}
            className={`flex gap-3 rounded-md border p-3 text-left transition-colors ${on ? "border-accent bg-accent-soft" : "border-line hover:border-accent"}`}
          >
            <m.icon className={`mt-0.5 size-4 shrink-0 ${on ? "text-accent" : "text-muted"}`} aria-hidden="true" />
            <span>
              <span className="block text-[13px] font-medium">{m.label}</span>
              <span className="mt-0.5 block text-xs text-muted">{m.text}</span>
            </span>
          </button>
        );
      })}
    </div>
  );
}
