import { Cable, KeyRound, Loader2, Plus, ShieldOff } from "lucide-react";
import { useState } from "react";

import { useRevokeTunnelGrant, useTunnelGrants, useTunnelHistory, useTunnels } from "../api/hooks";
import type { TunnelGrant } from "../api/types";
import { errorText, useToast } from "../app/toast";
import { useIsWorkspaceAdmin } from "../components/SettingsSections";
import { CreateGrantDialog } from "../components/tunnels/CreateGrant";
import { CannotOpenNote, Expires, KindChip, TunnelTable, useTargetLinks } from "../components/tunnels/parts";
import { Badge, Button, Confirm, Empty, ErrorNote, IconButton, Mono, PageHeader, Panel, Skeleton, Table, Td, ago, time } from "../components/ui";

type Tab = "open" | "history" | "grants";

/** Administration → Tunnels: everyone's open tunnels, the closed ones, and explicit grants. */
export function Tunnels() {
  const [tab, setTab] = useState<Tab>("open");
  const open = useTunnels(true);
  const history = useTunnelHistory(tab === "history");
  const grants = useTunnelGrants();
  const admin = useIsWorkspaceAdmin();
  const [creating, setCreating] = useState(false);
  const activeGrants = (grants.data?.grants ?? []).filter((g) => !g.revoked_at && (!g.expires_at || g.expires_at > Date.now()));

  const tabs: [Tab, string, number | undefined][] = [
    ["open", "Open", open.data?.supported ? open.data.tunnels.length : undefined],
    ["history", "History", undefined],
    ["grants", "Grants", grants.data?.supported ? activeGrants.length : undefined],
  ];

  return (
    <>
      <PageHeader
        eyebrow="Administration"
        title="Tunnels"
        description="Port tunnels opened with forge tunnel, and the grants that let people reach a service without a role on its environment."
        actions={
          tab === "grants" ? (
            <Button variant="primary" onClick={() => setCreating(true)} disabled={grants.data?.supported === false} title={grants.data?.supported === false ? "This server has no tunnel grants" : undefined}>
              <Plus className="size-4" /> Create grant
            </Button>
          ) : undefined
        }
      />
      <div className="mb-4 flex gap-1 border-b border-line" role="tablist">
        {tabs.map(([id, label, count]) => (
          <button
            key={id}
            role="tab"
            aria-selected={tab === id}
            onClick={() => setTab(id)}
            className={`press -mb-px inline-flex items-center gap-1.5 border-b-2 px-3 py-2 text-[13px] focus-visible:outline-2 focus-visible:outline-accent ${tab === id ? "border-accent text-accent" : "border-transparent text-muted hover:text-fg"}`}
          >
            {label}
            {count !== undefined && <span className="font-mono text-[11px] text-faint tabular-nums">{count}</span>}
          </button>
        ))}
      </div>
      <div key={tab} className="view-enter">
        {tab === "open" && (
          <Panel>
            <TunnelTable tunnels={open.data?.tunnels ?? []} loading={open.isLoading} supported={open.data?.supported ?? true} error={open.data?.error ?? null} showUser />
            <CannotOpenNote />
          </Panel>
        )}
        {tab === "history" && (
          <Panel>
            <TunnelTable
              tunnels={history.data?.tunnels ?? []}
              loading={history.isLoading}
              supported={history.data?.supported ?? true}
              error={history.data?.error ?? null}
              showUser
              history
              emptyHint="Closed tunnels appear here with the reason: closed by the user, idle, expired, grant revoked, sign-in ended or server restart."
            />
          </Panel>
        )}
        {tab === "grants" && <GrantsTab admin={admin} />}
      </div>
      <CreateGrantDialog open={creating} onOpenChange={setCreating} grants={grants.data} admin={admin} />
    </>
  );
}

function GrantsTab({ admin }: { admin: boolean }) {
  const q = useTunnelGrants();
  const revoke = useRevokeTunnelGrant();
  const toast = useToast();
  const links = useTargetLinks();
  const [revoking, setRevoking] = useState<TunnelGrant>();
  const [busyId, setBusyId] = useState<string>();
  const d = q.data;
  const rows = [...(d?.grants ?? [])].sort((a, b) => Number(!!a.revoked_at) - Number(!!b.revoked_at) || (b.created_at ?? 0) - (a.created_at ?? 0));

  return (
    <Panel
      description={
        d?.supported ? (
          <>
            A grant adds to roles: people with <Mono>developer</Mono> or <Mono>operator</Mono> on an environment can already tunnel to its services. Never-expiring grants{" "}
            {d.allow_never_expiry ? (
              <Badge tone="warn">allowed</Badge>
            ) : (
              <span title="Set [tunnels] allow_never_expiry = true in forge.toml to allow them.">
                <Badge>off</Badge>
              </span>
            )}
            {!admin && <span className="ml-1 text-faint">· raw node ports are for administrators only</span>}
          </>
        ) : undefined
      }
    >
      {q.isLoading ? (
        <Skeleton rows={3} />
      ) : d && !d.supported ? (
        <p className="px-4 py-3 text-xs text-muted">Not available on this server: update Forge to issue tunnel grants.</p>
      ) : d?.error ? (
        <ErrorNote error={d.error} />
      ) : rows.length === 0 ? (
        <Empty icon={<KeyRound className="size-5" />} title="No grants">
          Roles cover the usual cases. Create a grant for someone who needs one service, SSH on one node, or a raw port for a limited time.
        </Empty>
      ) : (
        <Table head={["User", "Target", "Node", "Ports", "Expires", "Created by", ""]}>
          {rows.map((g) => {
            const dead = !!g.revoked_at || (!!g.expires_at && g.expires_at < Date.now());
            return (
              <tr key={g.id} className={`text-[13px] ${dead ? "text-muted" : ""}`}>
                <Td className="max-w-[160px] truncate" title={g.user}>
                  {g.user || g.user_id.slice(0, 8) || "—"}
                </Td>
                <Td className="max-w-[260px]">
                  <span className="flex min-w-0 items-center gap-1.5">
                    <KindChip kind={g.target.kind} />
                    {links.target(g.target)}
                    {g.note && (
                      <span className="truncate text-[11px] text-faint" title={g.note}>
                        · {g.note}
                      </span>
                    )}
                  </span>
                </Td>
                <Td>{links.nodeLink(g.server_id, g.server)}</Td>
                <Td>
                  <span className="font-mono text-[11px] tabular-nums">{g.ports === "*" ? "all" : g.ports}</span>
                </Td>
                <Td>
                  {g.revoked_at ? (
                    <span title={time(g.revoked_at)}>
                      <Badge tone="bad">revoked {ago(g.revoked_at)}</Badge>
                    </span>
                  ) : (
                    <Expires at={g.expires_at} />
                  )}
                </Td>
                <Td className="max-w-[140px] truncate text-xs text-muted" title={`${g.created_by}${g.created_at ? ` · ${time(g.created_at)}` : ""}`}>
                  {g.created_by || "—"}
                  {g.created_at && <span className="text-faint"> · {ago(g.created_at)}</span>}
                </Td>
                <Td className="text-right">
                  {busyId === g.id ? (
                    <Loader2 className="inline size-4 animate-spin text-muted" aria-label="Revoking" />
                  ) : g.revoked_at ? null : (
                    <IconButton label={`Revoke grant for ${g.user}`} onClick={() => setRevoking(g)}>
                      <ShieldOff className="size-4" />
                    </IconButton>
                  )}
                </Td>
              </tr>
            );
          })}
        </Table>
      )}
      <Confirm
        open={!!revoking}
        onOpenChange={(o) => !o && setRevoking(undefined)}
        title="Revoke this grant?"
        body={
          revoking && (
            <div className="space-y-2 text-[13px]">
              <p>
                <b className="text-fg">{revoking.user}</b> loses access to <b className="text-fg">{revoking.target.label}</b>
                {revoking.server_id ? <> on <b className="text-fg">{links.nodeLabel(revoking.server_id, revoking.server)}</b></> : null}, unless a role on the environment still allows it.
              </p>
              <p className="flex items-center gap-1.5 text-xs text-muted">
                <Cable className="size-3.5" /> Tunnels open under this grant close within 30 seconds.
              </p>
            </div>
          )
        }
        confirmLabel="Revoke"
        busy={revoke.isPending}
        onConfirm={() => {
          const g = revoking;
          if (!g) return;
          setBusyId(g.id);
          setRevoking(undefined);
          revoke.mutateAsync(g.id).then(
            () => toast("Grant revoked"),
            (e) => toast(errorText(e), "error"),
          ).finally(() => setBusyId(undefined));
        }}
      />
    </Panel>
  );
}
