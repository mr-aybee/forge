import * as Popover from "@radix-ui/react-popover";
import { ChevronDown, Loader2, Minus, Plus } from "lucide-react";
import { useState } from "react";

import { type ServiceSwarmView, type SwarmTask, useServiceSwarmAction } from "../../api/swarm";
import { errorText, useToast } from "../../app/toast";
import { Button, IconButton } from "../ui";

/** How one replica is doing: drives its colour and animation. */
type Tone = "ok" | "warn" | "bad";

interface Replica {
  key: string;
  label: string;
  tone: Tone;
  state: string;
  node?: string;
  error?: string;
}

const FAILED = new Set(["failed", "rejected", "orphaned", "remove"]);

function toneOf(t: SwarmTask): Tone {
  if (FAILED.has(t.state)) return "bad";
  if (t.state === "running" && t.desired_state === "running") return "ok";
  return "warn";
}

/**
 * One entry per replica: replicated services by slot (1…desired), global
 * services by task. A slot with no live task is a missing replica (red).
 */
function replicas(v: ServiceSwarmView, hostOf: (id?: string) => string | undefined): Replica[] {
  const s = v.status;
  if (!s?.exists) return [];
  const wanted = s.tasks.filter((t) => t.desired_state === "running" || t.state === "running");
  if (v.settings.mode === "global") {
    return wanted.map((t) => ({ key: t.id, label: hostOf(t.node_id) ?? "node", tone: toneOf(t), state: t.state, node: hostOf(t.node_id), error: t.error }));
  }
  const out: Replica[] = [];
  for (let slot = 1; slot <= s.desired; slot++) {
    // The newest task of the slot (a replacement starts while the old one stops).
    const t = wanted.filter((x) => x.slot === slot).sort((a, b) => String(b.updated_at ?? "").localeCompare(String(a.updated_at ?? "")))[0];
    out.push(
      t
        ? { key: `s${slot}`, label: `#${slot}`, tone: toneOf(t), state: t.state, node: hostOf(t.node_id), error: t.error }
        : { key: `s${slot}`, label: `#${slot}`, tone: "bad", state: "missing" },
    );
  }
  return out;
}

const DOT: Record<Tone, string> = {
  ok: "bg-ok",
  warn: "bg-warn animate-pulse",
  bad: "bg-bad",
};

/** Green / orange / red dots, one per replica, with running count and an inline scale control. */
export function ReplicaBar({ v, serviceId, hostOf, canScale = true }: { v: ServiceSwarmView; serviceId: string; hostOf: (nodeId?: string) => string | undefined; canScale?: boolean }) {
  const list = replicas(v, hostOf);
  const s = v.status;
  const ok = list.filter((r) => r.tone === "ok").length;
  const global = v.settings.mode === "global";
  const [open, setOpen] = useState(false);
  const [want, setWant] = useState(s?.desired ?? 1);
  const act = useServiceSwarmAction(serviceId);
  const toast = useToast();
  if (!s?.exists) return <span className="text-xs text-faint">not in the swarm</span>;
  const overall: Tone = ok === list.length && list.length > 0 ? "ok" : list.some((r) => r.tone === "bad") ? "bad" : "warn";
  const apply = () =>
    act.mutate(
      { kind: "scale", replicas: want },
      { onSuccess: () => (toast(`Scaling to ${want}`), setOpen(false)), onError: (e) => toast(errorText(e), "error") },
    );
  return (
    <Popover.Root
      open={open}
      onOpenChange={(o) => {
        setOpen(o);
        if (o) setWant(s.desired);
      }}
    >
      <Popover.Trigger
        className="inline-flex h-7 max-w-full items-center gap-2 rounded-md border border-line bg-panel px-2 hover:border-accent focus-visible:outline-2 focus-visible:outline-accent"
        aria-label={`${ok} of ${list.length} replicas healthy; open replica details`}
      >
        <span className="flex min-w-0 flex-wrap items-center gap-1">
          {list.slice(0, 24).map((r) => (
            <span key={r.key} className={`dot-in size-2.5 rounded-full transition-colors duration-500 ${DOT[r.tone]}`} title={`${r.label}${r.node ? ` on ${r.node}` : ""}: ${r.state}`} />
          ))}
          {list.length > 24 && <span className="font-mono text-[10px] text-faint">+{list.length - 24}</span>}
        </span>
        <span className={`font-mono text-[11px] tabular-nums ${overall === "ok" ? "text-ok" : overall === "bad" ? "text-bad" : "text-warn"}`}>
          {ok}/{list.length}
        </span>
        <ChevronDown className={`size-3.5 text-faint transition-transform ${open ? "rotate-180" : ""}`} aria-hidden />
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content align="end" sideOffset={6} className="pop-in z-50 w-72 rounded-lg border border-line bg-panel p-3 text-xs shadow-[var(--shadow-pop)]">
          <div className="mb-2 flex items-center justify-between">
            <span className="text-muted">Running</span>
            <span className="font-mono tabular-nums">
              {s.running} / {s.desired}
            </span>
          </div>
          <ul className="mb-3 max-h-48 space-y-1 overflow-y-auto">
            {list.map((r) => (
              <li key={r.key} className="flex items-center gap-2">
                <span className={`size-2.5 shrink-0 rounded-full ${DOT[r.tone]}`} aria-hidden />
                <span className="font-mono text-faint">{r.label}</span>
                <span className="min-w-0 truncate">{r.node ?? "—"}</span>
                <span className={`ml-auto shrink-0 ${r.tone === "ok" ? "text-ok" : r.tone === "bad" ? "text-bad" : "text-warn"}`} title={r.error}>
                  {r.state}
                </span>
              </li>
            ))}
          </ul>
          {global ? (
            <p className="text-faint">Global: one task on every matching node.</p>
          ) : (
            <div className="flex items-center gap-2 border-t border-line pt-2.5">
              <span className="text-muted">Scale</span>
              <IconButton type="button" label="One replica fewer" className="size-7" disabled={!canScale || want <= 1} onClick={() => setWant((w) => Math.max(1, w - 1))}>
                <Minus className="size-3.5" />
              </IconButton>
              <span className="w-8 text-center font-mono tabular-nums">{want}</span>
              <IconButton type="button" label="One replica more" className="size-7" disabled={!canScale || want >= 1000} onClick={() => setWant((w) => Math.min(1000, w + 1))}>
                <Plus className="size-3.5" />
              </IconButton>
              <Button type="button" variant="primary" className="ml-auto" disabled={!canScale || want === s.desired || act.isPending} onClick={apply}>
                {act.isPending && <Loader2 className="size-3.5 animate-spin" />} Apply
              </Button>
            </div>
          )}
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}
