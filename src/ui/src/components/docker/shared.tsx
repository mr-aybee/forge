import { Link } from "@tanstack/react-router";
import { Boxes, Loader2 } from "lucide-react";
import { useState, type ReactNode } from "react";

import { type DockerInspectOp, type DockerOp, isDockerUnavailable, isMissingEndpoint, useDockerInspect, useDockerOp } from "../../api/dockerResources";
import { errorText, useToast } from "../../app/toast";
import { JobDetail } from "../JobsDrawer";
import { Button, CopyButton, Drawer, Empty, ErrorNote, Skeleton } from "../ui";

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

/** Inspect / history JSON of one object, pretty and copyable. Mount it only when open. */
export function InspectDrawer({ server, op, title, onClose }: { server: string; op: DockerInspectOp; title: string; onClose: () => void }) {
  const q = useDockerInspect(server, op);
  const text = q.data === undefined ? "" : JSON.stringify(q.data, null, 2);
  return (
    <Drawer open onOpenChange={(o) => !o && onClose()} title={title} wide>
      {q.error ? (
        <ErrorNote error={q.error} />
      ) : q.isLoading ? (
        <Skeleton rows={8} />
      ) : (
        <div className="space-y-2">
          <div className="flex justify-end">
            <CopyButton text={text} label="Copy JSON" />
          </div>
          <pre className="max-h-[calc(100vh-140px)] overflow-auto rounded-md border border-line bg-panel-2 p-3 font-mono text-[11px] leading-relaxed whitespace-pre select-text">{text}</pre>
        </div>
      )}
    </Drawer>
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
  return { run, runMany, bulk, busy, pending: m.isPending, view };
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
