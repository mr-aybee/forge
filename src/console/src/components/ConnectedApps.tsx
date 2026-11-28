import { useQuery } from "@tanstack/react-query";
import { Ban, Bot, Unplug } from "lucide-react";
import { useMemo, useState } from "react";

import {
  modeLabel,
  uriHost,
  useConnectedApps,
  useOAuthClients,
  useRevokeClient,
  useRevokeConnection,
  type Connection,
  type ConnectorClient,
} from "../api/connectors";
import { get } from "../api/client";
import { useMe } from "../api/hooks";
import type { User } from "../api/types";
import { errorText, useToast } from "../app/toast";
import { Badge, Confirm, CopyIcon, Empty, ErrorNote, Panel, RowMenu, type RowAction, Skeleton, Table, Td, ago, time, useRowMenu } from "./ui";

const When = ({ at, never = "—" }: { at: number | null; never?: string }) => (
  <span className="whitespace-nowrap" title={at ? time(at) : undefined}>
    {at ? ago(at) : never}
  </span>
);

const ModeBadge = ({ mode }: { mode: string }) => <Badge tone={mode === "auto" ? "warn" : mode === "read_only" ? "accent" : "muted"}>{modeLabel(mode)}</Badge>;

const StateBadge = ({ revoked }: { revoked: number | null }) =>
  revoked ? (
    <span title={`Revoked ${time(revoked)}`}>
      <Badge tone="muted">Revoked</Badge>
    </span>
  ) : (
    <Badge tone="ok">Active</Badge>
  );

/** Active first, newest first. */
const byState = <T extends { revoked_at: number | null; created_at: number | null }>(a: T, b: T) =>
  Number(!!a.revoked_at) - Number(!!b.revoked_at) || (b.created_at ?? 0) - (a.created_at ?? 0);

/**
 * Settings → Connected apps (issue #6): AI agents (Claude, ChatGPT, Cursor)
 * connected over MCP. Administrators can list everyone's connections.
 */
export function ConnectedAppsPanel() {
  const toast = useToast();
  const me = useMe().data;
  // `user.manage` holders get the clients list; everyone else gets 403 and no admin controls.
  const clients = useOAuthClients();
  const isAdmin = clients.data?.status === "ok";
  const [everyone, setEveryone] = useState(false);
  const showAll = everyone && isAdmin;
  const mine = useConnectedApps(false);
  const all = useConnectedApps(true, showAll);
  const q = showAll ? all : mine;
  // Names for the User column; only fetched for the everyone view (administrators).
  const users = useQuery({ queryKey: ["users"], queryFn: () => get<User[]>("/api/v1/users"), retry: false, enabled: showAll });
  const names = useMemo(() => {
    const m = new Map<string, string>();
    for (const u of Array.isArray(users.data) ? users.data : []) m.set(u.id, u.display_name || u.username || u.email || u.id);
    return m;
  }, [users.data]);
  const revoke = useRevokeConnection();
  const [revoking, setRevoking] = useState<Connection>();

  const res = q.data;
  const list = useMemo(() => [...(res?.connections ?? [])].sort(byState), [res]);
  const off = mine.data?.status === "off";
  const userOf = (id: string) => (id === me?.user.id ? "you" : (names.get(id) ?? id));
  const rm = useRowMenu();
  const connActions = (c: Connection): RowAction[] => [
    { label: "Revoke", icon: Unplug, danger: true, disabled: !!c.revoked_at, hint: c.revoked_at ? "Already revoked" : undefined, onSelect: () => setRevoking(c) },
  ];

  return (
    <Panel
      title="Connected apps"
      description="AI agents such as Claude, ChatGPT or Cursor that you allowed to use Forge over MCP. They act as you, within the mode you chose."
      actions={
        isAdmin && !off ? (
          <label className="flex cursor-pointer items-center gap-1.5 text-xs text-muted select-none">
            <input type="checkbox" className="accent-accent" checked={everyone} onChange={(e) => setEveryone(e.target.checked)} />
            Show everyone's
          </label>
        ) : undefined
      }
    >
      {q.isLoading ? (
        <Skeleton rows={2} />
      ) : off ? (
        <Empty icon={<Bot className="size-5" />} title="AI connectors are turned off">
          AI connectors are turned off on this Forge. An administrator can turn them on with <code className="font-mono text-xs">[mcp] enabled = true</code> in{" "}
          <code className="font-mono text-xs">forge.toml</code>.
        </Empty>
      ) : res && res.status !== "ok" ? (
        <ErrorNote error={res.error || "Could not load connected apps."} />
      ) : list.length === 0 ? (
        <Empty icon={<Bot className="size-5" />} title={showAll ? "No one has connected an app" : "No connected apps"}>
          Add Forge as a connector in your AI app with this Forge's <code className="font-mono text-xs">/mcp</code> URL. You approve each app here in the console.
        </Empty>
      ) : (
        <Table head={["App", ...(showAll ? ["User"] : []), "Mode", "Connected", "Last used", "Status", ""]}>
          {list.map((c) => (
            <tr key={c.id} className={c.revoked_at ? "text-muted" : undefined} {...rm.rowProps(connActions(c), `Actions for ${c.client_name}`)}>
              <Td className="max-w-[280px]">
                <div className="truncate text-[13px] font-medium" title={`${c.client_name} (${c.client_id})`}>
                  {c.client_name}
                </div>
                <div className="truncate font-mono text-[11px] text-faint" title={c.redirect_host}>
                  → {c.redirect_host || "—"}
                </div>
              </Td>
              {showAll && (
                <Td className="max-w-[180px] truncate text-xs" title={c.user_id}>
                  {c.user_id ? userOf(c.user_id) : "—"}
                </Td>
              )}
              <Td>
                <ModeBadge mode={c.mode} />
              </Td>
              <Td className="text-xs">
                <When at={c.created_at} />
              </Td>
              <Td className="text-xs">
                <When at={c.last_used_at} never="never" />
              </Td>
              <Td>
                <StateBadge revoked={c.revoked_at} />
              </Td>
              <Td className="w-10 text-right">
                <RowMenu label={`Actions for ${c.client_name}`} actions={connActions(c)} />
              </Td>
            </tr>
          ))}
        </Table>
      )}
      {rm.menu}

      <Confirm
        open={!!revoking}
        onOpenChange={(o) => !o && !revoke.isPending && setRevoking(undefined)}
        title={`Revoke ${revoking?.client_name ?? "this app"}?`}
        body={
          revoking ? (
            <>
              <strong className="text-fg">{revoking.client_name}</strong> (<span className="font-mono text-xs">{revoking.redirect_host || "—"}</span>)
              {revoking.user_id && revoking.user_id !== me?.user.id ? ` of ${userOf(revoking.user_id)}` : ""} loses access at once: its tokens stop
              working and calls in progress fail. The app can ask to connect again; that needs a new approval in the console.
            </>
          ) : (
            ""
          )
        }
        confirmLabel="Revoke"
        busy={revoke.isPending}
        onConfirm={() =>
          revoking &&
          revoke.mutate(revoking.id, {
            onSuccess: () => {
              toast(`${revoking.client_name} revoked`);
              setRevoking(undefined);
            },
            onError: (e) => toast(errorText(e), "error"),
          })
        }
      />
    </Panel>
  );
}

/**
 * Settings → Registered connectors (administrators): every app that
 * registered itself with Forge's OAuth server. Hidden for everyone else.
 */
export function RegisteredConnectorsPanel() {
  const toast = useToast();
  const clients = useOAuthClients();
  const isAdmin = clients.data?.status === "ok";
  const all = useConnectedApps(true, isAdmin);
  const revoke = useRevokeClient();
  const [revoking, setRevoking] = useState<ConnectorClient>();

  const active = useMemo(() => {
    const m = new Map<string, number>();
    for (const c of all.data?.connections ?? []) if (!c.revoked_at) m.set(c.client_id, (m.get(c.client_id) ?? 0) + 1);
    return m;
  }, [all.data]);
  const counted = all.data?.status === "ok";
  const list = useMemo(() => [...(clients.data?.clients ?? [])].sort(byState), [clients.data]);
  const rm = useRowMenu();
  const clientActions = (c: ConnectorClient): RowAction[] => [
    { label: "Revoke", icon: Ban, danger: true, disabled: !!c.revoked_at, hint: c.revoked_at ? "Already revoked" : undefined, onSelect: () => setRevoking(c) },
  ];

  if (!isAdmin) return null;
  const n = revoking ? (active.get(revoking.client_id) ?? 0) : 0;

  return (
    <Panel className="min-w-0 xl:col-span-2" title="Registered connectors" description="Apps that registered with this Forge's OAuth server. Registering grants nothing until someone approves a connection.">
      {list.length === 0 ? (
        <Empty icon={<Bot className="size-5" />} title="No connectors registered">
          An app registers itself the first time someone adds Forge as a connector.
        </Empty>
      ) : (
        <Table head={["Name given by the app", "Redirects to", "Connections", "Registered", "Last used", "Status", ""]}>
          {list.map((c) => {
            const hosts = [...new Set(c.redirect_uris.map(uriHost))];
            return (
              <tr key={c.client_id} className={c.revoked_at ? "text-muted" : undefined} {...rm.rowProps(clientActions(c), `Actions for ${c.name}`)}>
                <Td className="max-w-[260px]">
                  <div className="truncate text-[13px] font-medium" title={c.name}>
                    {c.name}
                  </div>
                  <div className="flex min-w-0 items-center gap-0.5">
                    <span className="truncate font-mono text-[11px] text-faint select-all" title={c.client_id}>
                      {c.client_id}
                    </span>
                    <CopyIcon text={c.client_id} label="Copy client ID" />
                  </div>
                </Td>
                <Td className="max-w-[240px]">
                  <div className="truncate font-mono text-[11px]" title={c.redirect_uris.join("\n") || undefined}>
                    {hosts.length ? hosts.join(", ") : "—"}
                  </div>
                </Td>
                <Td className="font-mono text-[11px] tabular-nums">{counted ? (active.get(c.client_id) ?? 0) : "—"}</Td>
                <Td className="text-xs">
                  <When at={c.created_at} />
                </Td>
                <Td className="text-xs">
                  <When at={c.last_used_at} never="never" />
                </Td>
                <Td>
                  <StateBadge revoked={c.revoked_at} />
                </Td>
                <Td className="w-10 text-right">
                  <RowMenu label={`Actions for ${c.name}`} actions={clientActions(c)} />
                </Td>
              </tr>
            );
          })}
        </Table>
      )}
      {clients.data?.status === "ok" && all.data?.status === "error" && <ErrorNote error={all.data.error} />}
      {rm.menu}

      <Confirm
        open={!!revoking}
        onOpenChange={(o) => !o && !revoke.isPending && setRevoking(undefined)}
        title={`Revoke ${revoking?.name ?? "this connector"}?`}
        body={
          revoking ? (
            <>
              Blocks this app and ends every connection made with it.{" "}
              {counted
                ? n > 0
                  ? `${n} active connection${n === 1 ? "" : "s"} stop${n === 1 ? "s" : ""} working at once.`
                  : "It has no active connections."
                : "Its connections stop working at once."}
            </>
          ) : (
            ""
          )
        }
        confirmLabel="Revoke connector"
        busy={revoke.isPending}
        onConfirm={() =>
          revoking &&
          revoke.mutate(revoking.client_id, {
            onSuccess: () => {
              toast(`${revoking.name} revoked`);
              setRevoking(undefined);
            },
            onError: (e) => toast(errorText(e), "error"),
          })
        }
      />
    </Panel>
  );
}
