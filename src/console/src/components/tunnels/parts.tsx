import { Link } from "@tanstack/react-router";
import { ArrowDown, ArrowUp, Cable, Loader2, X } from "lucide-react";
import { type ReactNode, useState } from "react";

import { useServices, useCloseTunnel, useServers } from "../../api/hooks";
import { useStacks } from "../../api/stacks";
import { nodeName, type Server, type ServicePort, type Tunnel, type TunnelTarget } from "../../api/types";
import { errorText, useToast } from "../../app/toast";
import { Badge, Confirm, CopyValue, Empty, ErrorNote, IconButton, Mono, Skeleton, Table, Td, ago, bytes, time } from "../ui";

export const TARGET_KINDS: Record<string, string> = { app: "Service", stack: "Stack service", package: "Database", ssh: "SSH", port: "Raw port" };

/** The `forge tunnel open …` line that opens a tunnel to `t` on `node` (name or id). */
export function tunnelCommand(t: TunnelTarget, node?: string | null, port?: number | null, kind = t.kind): string {
  const n = node ? ` --node ${node}` : "";
  switch (kind) {
    case "app":
      return `forge tunnel open --app ${t.ref}${n}`;
    case "stack": {
      const [stack, service] = t.ref.split("/");
      return `forge tunnel open --stack ${stack}${service ? ` --service ${service}` : ""}${port ? ` --port ${port}` : ""}${n}`;
    }
    case "package":
      return `forge tunnel open --db ${t.ref}${n}`;
    case "ssh":
      return `forge tunnel open${n} --ssh`;
    default:
      return `forge tunnel open${n}${port ? ` --port ${port}` : ""}`;
  }
}

/** The command for one catalogue entry of a service page. */
export function portCommand(e: ServicePort, kind: "app" | "stack" | "package", service: string, several: boolean): string {
  const node = e.server || e.server_id.slice(0, 8);
  if (kind === "stack") return tunnelCommand({ kind, ref: `${service}/${e.name}`, label: e.label }, node, several ? e.port : null);
  return tunnelCommand({ kind, ref: service, label: e.label }, node);
}

/** Resolves a target to the page it refers to; nodes are always linkable. */
export function useTargetLinks() {
  const apps = useServices().data ?? [];
  const stacks = useStacks().data ?? [];
  const nodes = useServers().data ?? [];
  const node = (id: string | null | undefined): Server | undefined => (id ? nodes.find((s) => s.id === id) : undefined);
  const nodeLabel = (id: string | null | undefined, fallback?: string | null) => {
    const n = node(id);
    return n ? nodeName(n) : fallback || (id ? id.slice(0, 8) : "—");
  };
  const target = (t: TunnelTarget) => {
    const label = t.label || t.ref || "—";
    const cls = "truncate hover:text-accent";
    if (t.kind === "app") {
      const a = apps.find((x) => x.name === t.ref || `${x.spec?.environment}/${x.name}` === t.ref);
      if (a)
        return (
          <Link to="/services/$serviceId" params={{ serviceId: a.id }} className={cls} title={label}>
            {label}
          </Link>
        );
    } else if (t.kind === "stack") {
      const name = t.ref.split("/")[0];
      const s = stacks.find((x) => x.stack.name === name || x.stack.id === name);
      if (s)
        return (
          <Link to="/stacks/$stackId" params={{ stackId: s.stack.id }} className={cls} title={label}>
            {label}
          </Link>
        );
    } else if (t.kind === "package" && t.ref) {
      return (
        <Link to="/app-store/$packageId" params={{ packageId: t.ref }} className={cls} title={label}>
          {label}
        </Link>
      );
    } else if (t.kind === "ssh" || t.kind === "port") {
      return <span title={label}>{t.kind === "ssh" ? "SSH" : label}</span>;
    }
    return (
      <span className="truncate" title={label}>
        {label}
      </span>
    );
  };
  const nodeLink = (id: string | null | undefined, fallback?: string | null) =>
    id ? (
      <Link to="/nodes/$nodeId" params={{ nodeId: id }} className="hover:text-accent" title={id}>
        {nodeLabel(id, fallback)}
      </Link>
    ) : (
      <span className="text-faint">any</span>
    );
  return { target, nodeLink, nodeLabel };
}

/** `↑ up · ↓ down` in binary units. */
export function Bytes({ up, down }: { up: number; down: number }) {
  return (
    <span className="inline-flex items-center gap-1 font-mono text-[11px] tabular-nums whitespace-nowrap" title={`${bytes(up)} sent to the node · ${bytes(down)} received`}>
      <ArrowUp className="size-3 text-faint" aria-hidden /> {bytes(up)}
      <span className="text-faint">·</span>
      <ArrowDown className="size-3 text-faint" aria-hidden /> {bytes(down)}
    </span>
  );
}

export function Expires({ at }: { at: number | null }) {
  if (!at) return <Badge tone="warn">never</Badge>;
  const past = at < Date.now();
  return (
    <span className={`text-xs ${past ? "text-bad" : "text-muted"}`} title={time(at)}>
      {past ? "expired" : ago(at)}
    </span>
  );
}

/** Kind chip in front of a target label. */
export function KindChip({ kind }: { kind: string }) {
  return <Badge tone={kind === "port" ? "warn" : "muted"}>{TARGET_KINDS[kind] ?? (kind || "—")}</Badge>;
}

/**
 * Open or closed tunnels, one line each. `history` adds the closed-at and
 * reason columns and drops Close; `showUser` adds the owner.
 */
export function TunnelTable({
  tunnels,
  loading,
  error,
  supported,
  showUser,
  history,
  emptyHint,
}: {
  tunnels: Tunnel[];
  loading: boolean;
  error: string | null;
  supported: boolean;
  showUser?: boolean;
  history?: boolean;
  emptyHint?: ReactNode;
}) {
  const links = useTargetLinks();
  const close = useCloseTunnel();
  const toast = useToast();
  const [closing, setClosing] = useState<Tunnel>();
  const [busyId, setBusyId] = useState<string>();

  if (loading) return <Skeleton rows={3} />;
  if (!supported) return <p className="px-4 py-3 text-xs text-muted">Not available on this server: update Forge to list {history ? "closed" : "open"} tunnels here.</p>;
  if (error) return <ErrorNote error={error} />;
  if (tunnels.length === 0)
    return (
      <Empty icon={<Cable className="size-5" />} title={history ? "No closed tunnels yet" : "No open tunnels"}>
        {emptyHint ?? (
          <>
            Open one with <Mono>forge tunnel open --app &lt;name&gt;</Mono>.
          </>
        )}
      </Empty>
    );

  const head = ["Target", "Node", "Remote", ...(showUser ? ["User"] : []), ...(history ? ["Opened", "Closed", "Reason"] : ["Local", "Opened", "Expires"]), "Bytes", ...(history ? [] : [""])];
  return (
    <>
      <Table head={head}>
        {tunnels.map((t) => (
          <tr key={t.id} className="text-[13px]">
            <Td className="max-w-[260px]">
              <span className="flex min-w-0 items-center gap-1.5">
                <KindChip kind={t.target.kind} />
                {links.target(t.target)}
              </span>
            </Td>
            <Td>{links.nodeLink(t.server_id, t.server)}</Td>
            <Td>
              <span className="font-mono text-[11px] tabular-nums">{t.port ?? "—"}</span>
              {t.role && <span className="ml-1.5 text-[11px] text-faint">{t.role}</span>}
            </Td>
            {showUser && (
              <Td className="max-w-[160px] truncate" title={t.user}>
                {t.user || "—"}
              </Td>
            )}
            {history ? (
              <>
                <Td className="text-xs text-muted" title={time(t.opened_at)}>
                  {t.opened_at ? ago(t.opened_at) : "—"}
                </Td>
                <Td className="text-xs text-muted" title={time(t.closed_at)}>
                  {t.closed_at ? ago(t.closed_at) : "—"}
                </Td>
                <Td>
                  <span className="font-mono text-[11px]">{t.close_reason ?? "—"}</span>
                </Td>
              </>
            ) : (
              <>
                <Td>
                  <CopyValue text={t.bind} label="local address" />
                </Td>
                <Td className="text-xs text-muted" title={time(t.opened_at)}>
                  {t.opened_at ? ago(t.opened_at) : "—"}
                </Td>
                <Td>
                  <Expires at={t.expires_at} />
                </Td>
              </>
            )}
            <Td>
              <Bytes up={t.bytes_up} down={t.bytes_down} />
            </Td>
            {!history && (
              <Td className="text-right">
                {busyId === t.id ? (
                  <Loader2 className="inline size-4 animate-spin text-muted" aria-label="Closing" />
                ) : (
                  <IconButton label={`Close tunnel to ${t.target.label}`} onClick={() => setClosing(t)}>
                    <X className="size-4" />
                  </IconButton>
                )}
              </Td>
            )}
          </tr>
        ))}
      </Table>
      <Confirm
        open={!!closing}
        onOpenChange={(o) => !o && setClosing(undefined)}
        title="Close this tunnel?"
        body={
          closing && (
            <p className="text-[13px]">
              Ends the tunnel from <Mono>{closing.bind || "the CLI"}</Mono> to <b className="text-fg">{closing.target.label}</b> on <b className="text-fg">{links.nodeLabel(closing.server_id, closing.server)}</b>
              {closing.user ? <> for <b className="text-fg">{closing.user}</b></> : null}. Its {closing.connections || "open"} connection{closing.connections === 1 ? "" : "s"} drop at once; the CLI can open it again.
            </p>
          )
        }
        confirmLabel="Close tunnel"
        busy={close.isPending}
        onConfirm={() => {
          const t = closing;
          if (!t) return;
          setBusyId(t.id);
          setClosing(undefined);
          close.mutateAsync(t.id).then(
            () => toast("Tunnel closed"),
            (e) => toast(errorText(e), "error"),
          ).finally(() => setBusyId(undefined));
        }}
      />
    </>
  );
}

/** One line: the console lists tunnels but cannot open one. */
export function CannotOpenNote() {
  return <p className="px-4 py-2 text-[11px] text-faint">The console cannot open a tunnel: a browser cannot listen on a local port. Open tunnels with the CLI; close them here or there.</p>;
}
