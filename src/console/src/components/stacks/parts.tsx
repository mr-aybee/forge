// Shared pieces of the stack views: policy and status chips, ports, the
// compose designer (services grouped by node) and the start checklist.
import { Link } from "@tanstack/react-router";
import { ArrowRight, CircleCheck, CircleX, Info, Server as ServerIcon, ShieldAlert, TriangleAlert, Upload } from "lucide-react";
import { Suspense, lazy, useCallback, useRef, type ReactNode } from "react";

import { useServers } from "../../api/hooks";
import { type Finding, type Policy, type Preview, type PreviewPort, type PreviewService, needsMfa, withDeps } from "../../api/stacks";
import { nodeName } from "../../api/types";
import { useStepUp } from "../../app/stepUp";
import { Badge, Skeleton } from "../ui";

const CodeEditor = lazy(() => import("../files/Editor").then((m) => ({ default: m.CodeEditor })));

/** The lazy CodeMirror YAML editor; `docKey` reloads `value` (e.g. after an upload). */
export function YamlEditor({ value, docKey, onChange, readOnly = false, height = "h-80" }: { value: string; docKey: string; onChange: (v: string) => void; readOnly?: boolean; height?: string }) {
  return (
    <div className={`${height} min-h-0 overflow-hidden rounded-md border border-line`}>
      <Suspense fallback={<Skeleton rows={6} />}>
        <CodeEditor value={value} docKey={docKey} lang="yaml" readOnly={readOnly} wrap={false} onChange={onChange} />
      </Suspense>
    </div>
  );
}

/** A small "Upload…" button that reads a text file (compose, .env). */
export function UploadText({ label, accept, onText }: { label: string; accept?: string; onText: (text: string, name: string) => void }) {
  const ref = useRef<HTMLInputElement>(null);
  return (
    <>
      <button type="button" onClick={() => ref.current?.click()} className="inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-xs text-muted hover:bg-panel-2 hover:text-fg">
        <Upload className="size-3.5" /> {label}
      </button>
      <input
        ref={ref}
        type="file"
        accept={accept}
        className="hidden"
        aria-label={label}
        onChange={(e) => {
          const f = e.target.files?.[0];
          e.target.value = "";
          if (!f) return;
          if (f.size > 1024 * 1024) return onText("", `${f.name} is larger than 1 MiB`);
          void f.text().then((t) => onText(t, f.name));
        }}
      />
    </>
  );
}

/** Runs `fn`; when the server says MFA is needed, asks for it once and retries. */
export function useMfaRetry() {
  const { ensureMfa } = useStepUp();
  return useCallback(
    async <T,>(fn: () => Promise<T>): Promise<T> => {
      try {
        return await fn();
      } catch (e) {
        if (needsMfa(e) && (await ensureMfa(e instanceof Error ? e.message : undefined))) return fn();
        throw e;
      }
    },
    [ensureMfa],
  );
}

// ------------------------------------------------------------------ chips

export function PolicyBadge({ policy }: { policy: Policy }) {
  if (policy === "blocked")
    return (
      <Badge tone="bad">
        <CircleX className="mr-1 size-3" /> blocked
      </Badge>
    );
  if (policy === "privileged")
    return (
      <span title="Deploying needs the stack.privileged permission (administrators) and an MFA-verified sign-in.">
        <Badge tone="warn">
          <TriangleAlert className="mr-1 size-3" /> needs privileged
        </Badge>
      </span>
    );
  return (
    <Badge tone="ok">
      <CircleCheck className="mr-1 size-3" /> allowed
    </Badge>
  );
}

const STATUS_TONE: Record<string, "ok" | "accent" | "muted" | "warn" | "bad"> = {
  deployed: "ok",
  running: "ok",
  healthy: "ok",
  failed: "bad",
  unhealthy: "bad",
  dead: "bad",
  exited: "muted",
  created: "muted",
  pending: "muted",
  paused: "warn",
  restarting: "warn",
  removing: "warn",
};

/** A node-part or container state with a tone; `*ing` states pulse. */
export function StateChip({ value }: { value: string | null | undefined }) {
  const v = value || "unknown";
  const moving = /ing$/.test(v) && v !== "running";
  const t = STATUS_TONE[v] ?? (moving ? "accent" : "muted");
  const cls = { ok: "bg-ok-soft text-ok", accent: "bg-accent-soft text-accent", muted: "bg-panel-2 text-muted", warn: "bg-warn-soft text-warn", bad: "bg-bad-soft text-bad" }[t];
  return (
    <span className={`inline-flex items-center gap-1.5 rounded px-1.5 py-0.5 text-xs font-medium whitespace-nowrap ${cls}`}>
      <span aria-hidden className={`size-1.5 rounded-full bg-current ${moving ? "animate-pulse" : ""}`} />
      {v}
    </span>
  );
}

/** `10.0.0.5:8080 → 80/tcp` with an internal/public chip. */
export function PortLine({ p }: { p: PreviewPort }) {
  const host = `${p.host_ip ? `${p.host_ip}:` : ""}${p.published || "auto"}`;
  return (
    <span className="inline-flex min-w-0 items-center gap-1.5">
      <span className="truncate font-mono text-[11px] tabular-nums" title={`${host} → ${p.target}/${p.protocol}`}>
        {host} <ArrowRight className="inline size-3 text-faint" /> {p.target}/{p.protocol}
      </span>
      {p.public ? <Badge tone="warn">public</Badge> : <Badge>internal</Badge>}
    </span>
  );
}

export function Findings({ list }: { list: Finding[] }) {
  const items = Array.isArray(list) ? list.filter((f) => f?.message) : [];
  if (!items.length) return null;
  return (
    <ul className="space-y-0.5">
      {items.map((f, i) => (
        <li key={i} className={`flex items-start gap-1.5 text-xs ${f.severity === "blocked" ? "text-bad" : f.severity === "privileged" ? "text-warn" : "text-muted"}`}>
          {f.severity === "blocked" ? <CircleX className="mt-0.5 size-3.5 shrink-0" /> : f.severity === "privileged" ? <ShieldAlert className="mt-0.5 size-3.5 shrink-0" /> : <Info className="mt-0.5 size-3.5 shrink-0" />}
          <span className="min-w-0 break-words">
            {f.service && <span className="font-mono text-[11px]">{f.service}: </span>}
            {f.message}
          </span>
        </li>
      ))}
    </ul>
  );
}

/** Node link with its display name (falls back to the short id). */
export function NodeLink({ id, fallback }: { id: string; fallback?: string }) {
  const node = (useServers().data ?? []).find((n) => n.id === id);
  if (!id) return <span className="text-faint">—</span>;
  return (
    <Link to="/nodes/$nodeId" params={{ nodeId: id }} className="inline-flex min-w-0 items-center gap-1 font-medium hover:text-accent">
      <ServerIcon className="size-3.5 shrink-0 text-faint" />
      <span className="truncate">{node ? nodeName(node) : fallback || id.slice(0, 8)}</span>
    </Link>
  );
}

// ------------------------------------------------------------------ designer

function Row({ k, children }: { k: string; children: ReactNode }) {
  return (
    <div className="grid grid-cols-[88px_minmax(0,1fr)] gap-2 py-0.5">
      <dt className="text-[11px] text-muted">{k}</dt>
      <dd className="min-w-0 text-xs">{children}</dd>
    </div>
  );
}

/** Whether a service joins the cluster container network, as the preview reports it. */
export function MeshValue({ mesh }: { mesh: boolean | string }) {
  if (mesh === true) return <Badge tone="ok">on</Badge>;
  if (mesh === false) return <span className="text-faint" title="The node is not on the cluster network">no</span>;
  return <Badge tone="muted">{mesh}</Badge>;
}

/** Reads top-level `x-forge.mesh` from a compose text: absent means on. */
export function composeMeshOn(text: string): boolean {
  const lines = text.split("\n");
  const i = lines.findIndex((l) => /^x-forge:\s*(#.*)?$/.test(l));
  if (i < 0) return !/^x-forge:\s*\{[^}]*\bmesh:\s*false\b/m.test(text);
  for (let j = i + 1; j < lines.length && (/^\s/.test(lines[j] ?? "") || lines[j] === ""); j++) {
    if (/^\s+mesh:\s*false\s*(#.*)?$/.test(lines[j] ?? "")) return false;
  }
  return true;
}

/** Sets top-level `x-forge.mesh`: on removes the key (the default), off writes `mesh: false`. Keeps the rest of the text. */
export function setComposeMesh(text: string, on: boolean): string {
  const lines = text.split("\n");
  const i = lines.findIndex((l) => /^x-forge:\s*(#.*)?$/.test(l));
  let end = i + 1;
  if (i >= 0) while (end < lines.length && (/^\s/.test(lines[end] ?? "") || lines[end] === "")) end++;
  const meshAt = i < 0 ? -1 : lines.slice(i + 1, end).findIndex((l) => /^\s+mesh:/.test(l));
  if (on) {
    if (meshAt < 0) return text.replace(/^x-forge:\s*\{\s*mesh:\s*false\s*\}\s*\n?/m, "");
    lines.splice(i + 1 + meshAt, 1);
    const rest = lines.slice(i + 1, end - 1).some((l) => l.trim() && !l.trim().startsWith("#"));
    if (!rest) lines.splice(i, end - 1 - i);
    return lines.join("\n");
  }
  if (i < 0) return `${text.replace(/\n*$/, "")}\n\nx-forge:\n  mesh: false\n`;
  if (meshAt >= 0) lines[i + 1 + meshAt] = (lines[i + 1 + meshAt] ?? "").replace(/mesh:.*/, "mesh: false");
  else lines.splice(i + 1, 0, "  mesh: false");
  return lines.join("\n");
}

/** The editor toggle for `x-forge.mesh`; `onChange` gets the rewritten compose text. */
export function MeshToggle({ content, onChange }: { content: string; onChange: (next: string) => void }) {
  const on = composeMeshOn(content);
  return (
    <label className="inline-flex cursor-pointer items-center gap-1.5 text-xs text-muted" title="Join the cluster container network so services reach each other across nodes. Writes x-forge.mesh in compose.yaml.">
      <input type="checkbox" className="accent-accent" checked={on} onChange={(e) => onChange(setComposeMesh(content, e.target.checked))} />
      Cluster network
    </label>
  );
}

function ServiceCard({ svc }: { svc: PreviewService }) {
  return (
    <div className="min-w-0 border-t border-line px-3 py-2 first:border-t-0">
      <div className="mb-1 flex min-w-0 flex-wrap items-center gap-2">
        <span className="truncate font-mono text-[13px] font-medium" title={svc.name}>
          {svc.name}
        </span>
        <PolicyBadge policy={svc.policy} />
        {svc.expose && <Badge tone="accent">expose {svc.expose}</Badge>}
      </div>
      <dl>
        <Row k="Image">
          <span className="block truncate font-mono text-[11px]" title={svc.image}>
            {svc.image || "—"}
          </span>
        </Row>
        {(svc.entrypoint || svc.command) && (
          <Row k="Command">
            <span className="block truncate font-mono text-[11px]" title={[svc.entrypoint, svc.command].filter(Boolean).join(" ")}>
              {[svc.entrypoint, svc.command].filter(Boolean).join(" ")}
            </span>
          </Row>
        )}
        <Row k="Ports">
          {svc.ports.length ? (
            <div className="space-y-0.5">
              {svc.ports.map((p, i) => (
                <div key={i}>
                  <PortLine p={p} />
                </div>
              ))}
            </div>
          ) : (
            <span className="text-faint">—</span>
          )}
        </Row>
        {svc.volumes.length > 0 && (
          <Row k="Volumes">
            {svc.volumes.map((v, i) => (
              <div key={i} className="truncate font-mono text-[11px]" title={v}>
                {v}
              </div>
            ))}
          </Row>
        )}
        {svc.environment.length > 0 && (
          <Row k="Env">
            <div className="flex flex-wrap gap-x-3 gap-y-0.5">
              {svc.environment.map((e, i) => (
                <span key={i} className="max-w-full truncate font-mono text-[11px]" title={`${e.key}=${e.value}`}>
                  {e.key}=<span className="text-muted">{e.value}</span>
                </span>
              ))}
            </div>
          </Row>
        )}
        {svc.env_file && (
          <Row k="Env file">
            <span className="font-mono text-[11px]">{svc.env_file}</span>
          </Row>
        )}
        {svc.depends_on.length > 0 && (
          <Row k="Depends on">
            <span className="font-mono text-[11px]">{svc.depends_on.join(", ")}</span>
          </Row>
        )}
        <Row k="Health">
          {svc.healthcheck ? (
            <span className="block truncate font-mono text-[11px]" title={svc.healthcheck.test}>
              {svc.healthcheck.test}
              {svc.healthcheck.interval && <span className="text-muted"> · every {svc.healthcheck.interval}</span>}
            </span>
          ) : (
            <span className="text-faint">image default</span>
          )}
        </Row>
        {svc.mesh !== undefined && (
          <Row k="Mesh">
            <MeshValue mesh={svc.mesh} />
          </Row>
        )}
        <Row k="Restart">
          <span className="font-mono text-[11px]">{svc.restart || "—"}</span>
        </Row>
      </dl>
      {svc.findings.length > 0 && (
        <div className="mt-1">
          <Findings list={svc.findings} />
        </div>
      )}
    </div>
  );
}

/** The compose preview: services grouped by node, with their policy result. */
export function Designer({ preview }: { preview: Preview }) {
  const services = Array.isArray(preview?.services) ? preview.services : [];
  const nodes = Array.isArray(preview?.nodes) ? preview.nodes : [];
  const by = new Map(services.map((x) => [x.name, x]));
  const placed = new Set(nodes.flatMap((n) => n.services));
  const orphans = services.filter((x) => !placed.has(x.name));
  return (
    <div className="space-y-2">
      {preview.findings.length > 0 && (
        <div className="rounded-md border border-line px-3 py-2">
          <Findings list={preview.findings} />
        </div>
      )}
      {nodes.map((n) => (
        <section key={n.server_id || n.hostname} className="min-w-0 rounded-md border border-line">
          <header className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 border-b border-line bg-panel-2/60 px-3 py-1.5">
            {n.server_id ? <NodeLink id={n.server_id} fallback={n.hostname} /> : <span className="font-medium">{n.hostname}</span>}
            <span className="font-mono text-[11px] text-muted tabular-nums" title="Address unmarked ports bind to">
              {n.internal_ip || "—"}
            </span>
            <span className="text-[11px] text-faint">
              {n.services.length} service{n.services.length === 1 ? "" : "s"}
            </span>
          </header>
          {n.findings.filter((f) => !f.service).length > 0 && (
            <div className="border-b border-line px-3 py-1.5">
              <Findings list={n.findings.filter((f) => !f.service)} />
            </div>
          )}
          {n.services.map((name) => {
            const svc = by.get(name);
            return svc ? <ServiceCard key={name} svc={svc} /> : null;
          })}
        </section>
      ))}
      {orphans.length > 0 && (
        <section className="rounded-md border border-line">
          <header className="border-b border-line bg-panel-2/60 px-3 py-1.5 text-xs text-muted">Not placed on a node</header>
          {orphans.map((svc) => (
            <ServiceCard key={svc.name} svc={svc} />
          ))}
        </section>
      )}
      {services.length === 0 && <p className="text-xs text-muted">The file defines no services.</p>}
    </div>
  );
}

/** Checkbox per service; ticking one shows its dependencies as auto-included. */
export function ServicePicker({ services, ticked, onChange }: { services: PreviewService[]; ticked: Set<string>; onChange: (s: Set<string>) => void }) {
  const list = Array.isArray(services) ? services : [];
  const all = withDeps(ticked, list);
  if (!list.length) return null;
  return (
    <div className="rounded-md border border-line">
      <div className="flex items-center justify-between border-b border-line px-3 py-1.5 text-xs">
        <span className="font-medium">Services to start</span>
        <span className="flex gap-2 text-muted">
          <button type="button" className="hover:text-accent" onClick={() => onChange(new Set(list.map((x) => x.name)))}>
            all
          </button>
          <button type="button" className="hover:text-accent" onClick={() => onChange(new Set())}>
            none
          </button>
        </span>
      </div>
      <ul className="max-h-48 divide-y divide-line overflow-y-auto">
        {list.map((x) => {
          const auto = !ticked.has(x.name) && all.has(x.name);
          return (
            <li key={x.name} className="flex min-w-0 items-center gap-2 px-3 py-1">
              <input
                id={`svc-${x.name}`}
                type="checkbox"
                checked={ticked.has(x.name) || auto}
                disabled={x.policy === "blocked" || auto}
                title={auto ? "Started because a ticked service depends on it" : undefined}
                onChange={(e) => {
                  const next = new Set(ticked);
                  if (e.target.checked) next.add(x.name);
                  else next.delete(x.name);
                  onChange(next);
                }}
              />
              <label htmlFor={`svc-${x.name}`} className="min-w-0 flex-1 truncate font-mono text-xs" title={x.image}>
                {x.name}
                <span className="ml-2 text-faint">{x.image}</span>
              </label>
              {auto && <Badge tone="accent">dependency</Badge>}
              {x.policy !== "allowed" && <PolicyBadge policy={x.policy} />}
            </li>
          );
        })}
      </ul>
      <p className="border-t border-line px-3 py-1 text-[11px] text-faint">
        {ticked.size === 0 ? "None ticked: every service starts." : `${all.size} of ${list.length} start (ticked plus their dependencies).`}
      </p>
    </div>
  );
}

/** Why starting is not possible (blocked, or privileged without the permission), or null. */
export function startBlocker(p: Preview | null | undefined): string | null {
  if (!p) return null;
  if (p.blocked) return "A service is blocked by policy; fix the findings before starting.";
  if (p.privileged && p.can_privileged === false) return "This stack needs privileged access. An administrator with stack.privileged and an MFA-verified sign-in must deploy it; you can still save it.";
  return null;
}
