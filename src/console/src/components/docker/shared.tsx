import { Link } from "@tanstack/react-router";
import { Boxes, Loader2 } from "lucide-react";
import { useState, type ReactNode } from "react";

import {
  type DockerImageDetail,
  type DockerInspectOp,
  type DockerNetwork,
  type DockerOp,
  type DockerVolume,
  isDockerUnavailable,
  isMissingEndpoint,
  normImageDetail,
  normNetwork,
  normVolume,
  useDockerInspect,
  useDockerOp,
} from "../../api/dockerResources";
import { ApiError } from "../../api/client";
import { errorText, useToast } from "../../app/toast";
import { JobDetail } from "../JobsDrawer";
import { ago, Badge, Button, bytes, CopyButton, CopyIcon, Drawer, Empty, ErrorNote, Segmented, Skeleton, time } from "../ui";
import { CodeView } from "../CodeField";

/** Why a list could not load: Docker missing, an older server, or the plain error. */
export function DockerProblem({ error, server }: { error: unknown; server: string }) {
  if (!error) return null;
  if (isDockerUnavailable(error)) {
    return (
      <Empty
        icon={<Boxes className="size-5" />}
        title="Docker Engine is not installed or not reachable"
        action={
          <Link to="/app-store" search={{ node: server }}>
            <Button variant="primary">
              <Boxes className="size-4" /> Install Docker Engine
            </Button>
          </Link>
        }
      >
        <span className="block">Install it from the App Store, or start the Docker service on the node.</span>
        <span className="mt-1 block truncate font-mono text-[11px] text-faint" title={errorText(error)}>
          {errorText(error)}
        </span>
      </Empty>
    );
  }
  if (isMissingEndpoint(error)) return <ErrorNote error="This Forge server does not offer Docker images, volumes and networks yet. Update Forge to use them." />;
  return <ErrorNote error={error} />;
}

/** Inspect / history of one object: a readable view where the server returns a typed shape, with a raw JSON toggle. Mount it only when open. */
export function InspectDrawer({ server, op, title, onClose }: { server: string; op: DockerInspectOp; title: string; onClose: () => void }) {
  const q = useDockerInspect(server, op);
  const [raw, setRaw] = useState(op.op === "image_history");
  const text = q.data === undefined ? "" : JSON.stringify(q.data, null, 2);
  const details = op.op === "image_history" ? null : <InspectDetails op={op} data={q.data} />;
  return (
    <Drawer open onOpenChange={(o) => !o && onClose()} title={title} wide>
      {q.error ? (
        <ErrorNote error={q.error} />
      ) : q.isLoading ? (
        <Skeleton rows={8} />
      ) : (
        <div className="space-y-2">
          <div className="flex items-center justify-end gap-2">
            {details && (
              <Segmented
                label="Inspect view"
                value={raw ? "raw" : "details"}
                options={[
                  { id: "details", label: "Details" },
                  { id: "raw", label: "Raw JSON" },
                ]}
                onChange={(v) => setRaw(v === "raw")}
              />
            )}
            <CopyButton text={text} label="Copy JSON" />
          </div>
          {details && !raw ? (
            details
          ) : (
            <CodeView lang="json" aria-label="Raw JSON" minLines={12} maxLines={44} value={text} />
          )}
        </div>
      )}
    </Drawer>
  );
}

function InspectDetails({ op, data }: { op: DockerInspectOp; data: unknown }) {
  if (op.op === "image_inspect") return <ImageDetails d={normImageDetail(data)} />;
  if (op.op === "volume_inspect") return <VolumeDetails v={normVolume(data)} />;
  if (op.op === "network_inspect") return <NetworkDetails n={normNetwork(data)} />;
  return null;
}

const dash = <span className="text-faint">—</span>;

/** One label/value line of a details list. */
function KV({ k, children }: { k: string; children: ReactNode }) {
  return (
    <div className="grid grid-cols-[minmax(90px,140px)_minmax(0,1fr)] gap-3 border-b border-line px-3 py-1.5 last:border-b-0">
      <dt className="text-xs text-muted">{k}</dt>
      <dd className="min-w-0 text-[13px] break-words">{children}</dd>
    </div>
  );
}

/** A selectable monospace value with a copy icon. */
function MonoCopy({ text, label }: { text?: string; label: string }) {
  if (!text) return dash;
  return (
    <span className="inline-flex max-w-full min-w-0 items-center gap-1.5">
      <span className="truncate font-mono text-[11px] select-all" title={text}>
        {text}
      </span>
      <CopyIcon text={text} label={label} />
    </span>
  );
}

const List = ({ items }: { items: string[] }) =>
  items.length ? (
    <ul className="space-y-0.5">
      {items.map((x, i) => (
        <li key={`${i}:${x}`} className="truncate font-mono text-[11px] select-text" title={x}>
          {x}
        </li>
      ))}
    </ul>
  ) : (
    dash
  );

const Section = ({ title, children }: { title: string; children: ReactNode }) => (
  <section>
    <h3 className="mb-1 text-[11px] font-semibold tracking-wide text-muted uppercase">{title}</h3>
    <dl className="rounded-md border border-line">{children}</dl>
  </section>
);

const Labels = ({ labels: l }: { labels: Record<string, string> }) => {
  const e = Object.entries(l);
  return e.length ? (
    <Section title={`Labels (${e.length})`}>
      {e.map(([k, v]) => (
        <KV key={k} k={k}>
          <span className="font-mono text-[11px] select-text">{v || "—"}</span>
        </KV>
      ))}
    </Section>
  ) : null;
};

const argv = (a: string[]) => (a.length ? <MonoCopy text={a.map((x) => (/\s/.test(x) ? JSON.stringify(x) : x)).join(" ")} label="Copy" /> : dash);

function ImageDetails({ d }: { d: DockerImageDetail }) {
  return (
    <div className="space-y-4">
      <Section title="Image">
        <KV k="ID">
          <MonoCopy text={d.full_id || d.id} label="Copy image id" />
        </KV>
        <KV k="Tags">
          <List items={d.tags} />
        </KV>
        <KV k="Digests">
          <List items={d.digests} />
        </KV>
        <KV k="Created">{d.created ? <span title={time(d.created)}>{ago(d.created)}</span> : dash}</KV>
        <KV k="Size">{d.size !== undefined ? <span className="font-mono text-[11px] tabular-nums">{bytes(d.size)}</span> : dash}</KV>
        <KV k="Platform">{d.os || d.architecture ? <span className="font-mono text-[11px]">{[d.os, d.architecture].filter(Boolean).join("/")}</span> : dash}</KV>
        <KV k="Author">{d.author ? <span className="break-all">{d.author}</span> : dash}</KV>
      </Section>
      <Section title="Run">
        <KV k="Entrypoint">{argv(d.entrypoint)}</KV>
        <KV k="Command">{argv(d.cmd)}</KV>
        <KV k="Working dir">
          <MonoCopy text={d.working_dir} label="Copy working directory" />
        </KV>
        <KV k="User">{d.user ? <span className="font-mono text-[11px]">{d.user}</span> : dash}</KV>
        <KV k="Exposed ports">
          {d.exposed_ports.length ? (
            <span className="flex flex-wrap gap-1">
              {d.exposed_ports.map((p) => (
                <Badge key={p}>{p}</Badge>
              ))}
            </span>
          ) : (
            dash
          )}
        </KV>
        <KV k="Volumes">
          <List items={d.volumes} />
        </KV>
      </Section>
      {d.env.length > 0 && (
        <Section title={`Environment (${d.env.length})`}>
          {d.env.map((e) => (
            <KV key={e.key} k={e.key}>
              {e.value === null ? (
                <span title="You need container.inspect_env to see values">
                  <Badge tone="warn">hidden</Badge>
                </span>
              ) : (
                <span className="font-mono text-[11px] break-all select-text">{e.value || "—"}</span>
              )}
            </KV>
          ))}
        </Section>
      )}
      <Labels labels={d.labels} />
      {d.layers.length > 0 && (
        <Section title={`Layers (${d.layers.length})`}>
          <div className="px-3 py-1.5">
            <List items={d.layers} />
          </div>
        </Section>
      )}
    </div>
  );
}

function VolumeDetails({ v }: { v: DockerVolume }) {
  return (
    <div className="space-y-4">
      <Section title="Volume">
        <KV k="Name">
          <MonoCopy text={v.name} label="Copy volume name" />
        </KV>
        <KV k="Driver">
          {v.driver}
          {v.scope && <span className="text-faint"> · {v.scope}</span>}
        </KV>
        <KV k="Mountpoint">
          <MonoCopy text={v.mountpoint} label="Copy mountpoint" />
        </KV>
        <KV k="Created">{v.created ? <span title={time(v.created)}>{ago(v.created)}</span> : dash}</KV>
        <KV k="Managed by">{v.managed ? <Badge tone="accent">Forge</Badge> : dash}</KV>
      </Section>
      <Labels labels={v.labels} />
    </div>
  );
}

function NetworkDetails({ n }: { n: DockerNetwork }) {
  return (
    <Section title="Network">
      <KV k="Name">
        <MonoCopy text={n.name} label="Copy network name" />
      </KV>
      <KV k="ID">
        <MonoCopy text={n.id} label="Copy network id" />
      </KV>
      <KV k="Driver">
        {n.driver || "—"}
        {n.scope && <span className="text-faint"> · {n.scope}</span>}
      </KV>
      <KV k="Subnets">
        <List items={n.subnets} />
      </KV>
      <KV k="Flags">
        {n.internal || n.attachable || n.protected ? (
          <span className="flex flex-wrap gap-1">
            {n.internal && <Badge>internal</Badge>}
            {n.attachable && <Badge>attachable</Badge>}
            {n.protected && <Badge tone="warn">protected</Badge>}
          </span>
        ) : (
          dash
        )}
      </KV>
      <KV k="Containers">
        <span className="font-mono text-[11px] tabular-nums">{n.containers}</span>
      </KV>
    </Section>
  );
}

/** A stable key for "which button is busy" (op + its subject). */
export const opKey = (op: DockerOp) => {
  const o = op as Record<string, unknown>;
  return `${op.op}:${String(o.id ?? o.name ?? o.image ?? o.network ?? "")}`;
};

/**
 * Runs Docker operations with feedback: a success toast, `errorText` on
 * failure, and for jobs a toast that opens the job's live log.
 */
export function useDockerRunner(server: string) {
  const m = useDockerOp(server);
  const toast = useToast();
  const [job, setJob] = useState<string>();
  const [bulk, setBulk] = useState(false);
  const busy = m.isPending && m.variables ? opKey(m.variables) : undefined;
  const run = (op: DockerOp, ok: string, after?: () => void) =>
    m.mutateAsync(op).then(
      (r) => {
        if (r.kind === "job") toast(`${ok}: started`, "ok", { label: "Follow job", onSelect: () => setJob(r.job.id) });
        else toast(ok);
        after?.();
        return true;
      },
      (e: unknown) => {
        toast(errorText(e), "error");
        return false;
      },
    );
  /**
   * Starts a push. Resolves `true` when the job started, or the message to show
   * in the dialog when it failed (400: no registry for that host).
   */
  const runPush = (image: string): Promise<true | string> =>
    m.mutateAsync({ op: "image_push", image }).then(
      (r) => {
        if (r.kind === "job") toast(`Push ${image}: started`, "ok", { label: "Follow job", onSelect: () => setJob(r.job.id) });
        else toast(`Push ${image}`);
        return true as const;
      },
      (e: unknown) => {
        if (e instanceof ApiError && e.status === 400) return e.message || "No registry in Settings → Registries matches this image's host.";
        return errorText(e);
      },
    );
  /** Runs quick operations one after another; one toast with the tally. */
  const runMany = async (ops: DockerOp[], noun: string, after?: () => void) => {
    setBulk(true);
    let ok = 0;
    const failed: string[] = [];
    for (const op of ops) {
      try {
        await m.mutateAsync(op);
        ok++;
      } catch (e) {
        failed.push(errorText(e));
      }
    }
    if (failed.length === 0) toast(`Removed ${ok} ${noun}`);
    else toast(`Removed ${ok} of ${ops.length} ${noun}; ${failed.length} failed: ${failed[0]}`, "error");
    setBulk(false);
    after?.();
  };
  const view: ReactNode = job ? (
    <Drawer open onOpenChange={(o) => !o && setJob(undefined)} title="Docker job" wide>
      <JobDetail id={job} onNavigate={() => setJob(undefined)} />
    </Drawer>
  ) : null;
  return { run, runPush, runMany, bulk, busy, pending: m.isPending, view };
}

export type DockerRunner = ReturnType<typeof useDockerRunner>;

/** Spinner when this op is the one running, else the icon. */
export const Spin = ({ on, children }: { on: boolean; children: ReactNode }) => (on ? <Loader2 className="size-4 animate-spin" /> : <>{children}</>);

/** A dense toolbar row above a table. */
export const Toolbar = ({ children }: { children: ReactNode }) => <div className="flex flex-wrap items-center gap-2 border-b border-line px-4 py-2">{children}</div>;

/** Header checkbox / row checkbox. */
export function Check({ checked, onChange, label, disabled, title }: { checked: boolean; onChange: (v: boolean) => void; label: string; disabled?: boolean; title?: string }) {
  return (
    <input
      type="checkbox"
      className="size-3.5 accent-accent disabled:opacity-40"
      aria-label={label}
      title={title ?? label}
      checked={checked}
      disabled={disabled}
      onChange={(e) => onChange(e.target.checked)}
    />
  );
}
