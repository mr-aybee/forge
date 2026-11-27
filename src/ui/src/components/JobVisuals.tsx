// Colour and icons for jobs: kind chips tinted by category, status icons and
// accents, a key/value card for known results, highlighted JSON.

import {
  Activity,
  Ban,
  Boxes,
  CircleCheck,
  CircleX,
  Clock,
  Globe,
  Loader2,
  Network,
  Package,
  Rocket,
  Server,
  ShieldCheck,
  Terminal,
  TriangleAlert,
  Workflow,
} from "lucide-react";
import type { ComponentType, ReactElement } from "react";

import type { Job } from "../api/types";
import { type JobCategory, isBackground, jobCategory } from "./jobMeta";
import { CopyIcon } from "./ui";

const CATEGORY: Record<JobCategory, { icon: ComponentType<{ className?: string }>; cls: string; label: string }> = {
  ingress: { icon: Network, cls: "bg-sky-soft text-sky", label: "Ingress" },
  network: { icon: Globe, cls: "bg-sky-soft text-sky", label: "Network" },
  site: { icon: ShieldCheck, cls: "bg-teal-soft text-teal", label: "Sites and TLS" },
  package: { icon: Package, cls: "bg-violet-soft text-violet", label: "Package" },
  container: { icon: Boxes, cls: "bg-indigo-soft text-indigo", label: "Containers" },
  health: { icon: Activity, cls: "bg-ok-soft text-ok", label: "Facts and health" },
  deploy: { icon: Rocket, cls: "bg-pink-soft text-pink", label: "Deploy" },
  agent: { icon: Terminal, cls: "bg-orange-soft text-orange", label: "Agent" },
  node: { icon: Server, cls: "bg-accent-soft text-accent", label: "Node" },
  other: { icon: Workflow, cls: "bg-panel-2 text-muted", label: "Job" },
};

/** Kind icon in a tinted circle. */
export function KindIcon({ job, size = "sm" }: { job: Job; size?: "sm" | "lg" }) {
  const c = CATEGORY[jobCategory(job)] ?? CATEGORY.other;
  const Icon = c.icon;
  return (
    <span
      aria-hidden
      title={c.label}
      className={`grid shrink-0 place-items-center rounded-full ${c.cls} ${size === "lg" ? "size-9" : "size-7"}`}
    >
      <Icon className={size === "lg" ? "size-4.5" : "size-3.5"} />
    </span>
  );
}

export type Tone = "ok" | "bad" | "warn" | "accent" | "muted";

/** Tone of a job's status; a failed background check is amber, not red. */
export function jobTone(j: Pick<Job, "status"> & Partial<Job>): Tone {
  switch (j.status) {
    case "succeeded":
      return "ok";
    case "failed":
      return j.kind && isBackground(j as Job) ? "warn" : "bad";
    case "running":
      return "accent";
    case "queued":
      return "muted";
    default:
      return "muted";
  }
}

/** Human status, with "check failed" for background probes. */
export function statusLabel(j: Job): string {
  if (j.status === "failed" && isBackground(j)) return "check failed";
  return j.status || "unknown";
}

export const toneText: Record<Tone, string> = { ok: "text-ok", bad: "text-bad", warn: "text-warn", accent: "text-accent", muted: "text-muted" };
export const toneBorder: Record<Tone, string> = {
  ok: "border-l-ok",
  bad: "border-l-bad",
  warn: "border-l-warn",
  accent: "border-l-accent",
  muted: "border-l-line-strong",
};
export const toneSoft: Record<Tone, string> = {
  ok: "bg-ok-soft text-ok",
  bad: "bg-bad-soft text-bad",
  warn: "bg-warn-soft text-warn",
  accent: "bg-accent-soft text-accent",
  muted: "bg-panel-2 text-muted",
};
export const toneBand: Record<Tone, string> = {
  ok: "border-ok/30 bg-ok-soft",
  bad: "border-bad/40 bg-bad-soft",
  warn: "border-warn/40 bg-warn-soft",
  accent: "border-accent/30 bg-accent-soft",
  muted: "border-line bg-panel-2",
};

/** Status icon in the job's tone; spins while running. */
export function StatusIcon({ job, className = "size-3.5" }: { job: Job; className?: string }) {
  const t = toneText[jobTone(job)];
  switch (job.status) {
    case "succeeded":
      return <CircleCheck aria-hidden className={`${className} ${t}`} />;
    case "failed":
      return isBackground(job) ? <TriangleAlert aria-hidden className={`${className} ${t}`} /> : <CircleX aria-hidden className={`${className} ${t}`} />;
    case "running":
      return <Loader2 aria-hidden className={`${className} animate-spin ${t}`} />;
    case "cancelled":
      return <Ban aria-hidden className={`${className} ${t}`} />;
    default:
      return <Clock aria-hidden className={`${className} ${t}`} />;
  }
}

/** Status pill in the job's tone. */
export function JobStatusPill({ job }: { job: Job }) {
  return (
    <span className={`inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[11px] font-medium whitespace-nowrap ${toneSoft[jobTone(job)]}`}>
      <StatusIcon job={job} className="size-3" />
      {statusLabel(job)}
    </span>
  );
}

// ------------------------------------------------------------------ results

const valueTone = (v: unknown): Tone => {
  if (v === true || v === "running" || v === "ok" || v === "active" || v === "healthy" || v === "valid") return "ok";
  if (v === false || v === "not-installed" || v === "absent" || v === null) return "muted";
  if (v === "stopped" || v === "inactive" || v === "degraded" || v === "expiring") return "warn";
  if (v === "failed" || v === "error" || v === "expired" || v === "unhealthy") return "bad";
  return "muted";
};

const isScalar = (v: unknown) => v === null || ["string", "number", "boolean"].includes(typeof v);

/** Result object worth a card: a flat object of scalars and scalar lists. */
export function resultFields(output: unknown): [string, unknown][] {
  if (!output || typeof output !== "object" || Array.isArray(output)) return [];
  const entries = Object.entries(output as Record<string, unknown>).filter(
    ([, v]) => isScalar(v) || (Array.isArray(v) && v.length <= 20 && v.every(isScalar)),
  );
  return entries.length >= 1 && entries.length <= 24 ? entries : [];
}

function Value({ v }: { v: unknown }) {
  if (Array.isArray(v))
    return v.length ? (
      <span className="flex min-w-0 flex-wrap gap-1">
        {v.map((x, i) => (
          <span key={i} className="rounded bg-panel-2 px-1.5 py-0.5 font-mono text-[11px]">
            {String(x)}
          </span>
        ))}
      </span>
    ) : (
      <span className="text-faint">—</span>
    );
  if (typeof v === "boolean" || typeof v === "string" || v === null) {
    const tone = valueTone(v);
    if (typeof v === "string" && tone === "muted" && v !== "not-installed")
      return (
        <span className="min-w-0 truncate font-mono text-[11px]" title={v}>
          {v || "—"}
        </span>
      );
    return <span className={`rounded px-1.5 py-0.5 text-[11px] font-medium ${toneSoft[tone]}`}>{v === null ? "—" : String(v === true ? "yes" : v === false ? "no" : v)}</span>;
  }
  return <span className="font-mono text-[11px] tabular-nums">{String(v)}</span>;
}

/** Key/value card for a flat JSON result (e.g. ingress status). */
export function ResultCard({ fields }: { fields: [string, unknown][] }) {
  return (
    <dl className="grid grid-cols-1 gap-x-4 gap-y-1.5 rounded-md border border-line px-3 py-2 sm:grid-cols-2">
      {fields.map(([k, v]) => (
        <div key={k} className="flex min-w-0 items-center gap-2">
          <dt className="w-28 shrink-0 truncate text-[11px] text-faint" title={k}>
            {k.replace(/_/g, " ")}
          </dt>
          <dd className="flex min-w-0 items-center text-xs">
            <Value v={v} />
          </dd>
        </div>
      ))}
    </dl>
  );
}

// ------------------------------------------------------------------ JSON

const TOKEN = /("(?:\\.|[^"\\])*")(\s*:)?|\b(true|false|null)\b|(-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?)/g;

function highlight(text: string) {
  const out: (string | ReactElement)[] = [];
  let last = 0;
  let i = 0;
  for (const m of text.matchAll(TOKEN)) {
    const at = m.index ?? 0;
    if (at > last) out.push(text.slice(last, at));
    const cls = m[1] ? (m[2] ? "text-accent" : "text-ok") : m[3] ? "text-violet" : "text-orange";
    out.push(
      <span key={i++} className={cls}>
        {m[1] ?? m[0]}
      </span>,
    );
    if (m[1] && m[2]) out.push(m[2]);
    last = at + m[0].length;
  }
  if (last < text.length) out.push(text.slice(last));
  return out;
}

/** Pretty JSON with syntax colours and a copy button. */
export function JsonView({ value }: { value: unknown }) {
  let text: string;
  try {
    text = JSON.stringify(value ?? {}, null, 2) ?? "";
  } catch {
    text = String(value);
  }
  // Very large payloads skip highlighting to keep the drawer responsive.
  const body = text.length > 60_000 ? text : highlight(text);
  return (
    <div className="relative">
      <pre className="max-h-72 overflow-auto rounded-md border border-line bg-panel-2 p-2.5 pr-8 font-mono text-[11px] leading-relaxed select-text">{body}</pre>
      <CopyIcon text={text} label="Copy JSON" className="absolute top-1.5 right-1.5" />
    </div>
  );
}
