import { Link, useNavigate, useSearch } from "@tanstack/react-router";
import { Cable, Database, Layers, PackageCheck, Pause, Play, Plus, Trash2, Unplug } from "lucide-react";
import { useMemo, useState } from "react";

import { ApiError } from "../api/client";
import { isAdopted, storeBusy, useObjectStores, useStoreAction } from "../api/storage";
import { errorText, useToast } from "../app/toast";
import type { ObjectStore } from "../api/types";
import { AdoptStoreDialog, ConnectStoreDialog, InstallStoreDialog, RemoveStoreDialog, providerLabel } from "../components/storage/StoreDialogs";
import { FoundOnNodes } from "../components/storage/FoundOnNodes";
import { ChangeEnvironmentDialog, StoreOwner } from "../components/storage/StoreOwner";
import { Badge, Button, Confirm, Empty, ErrorNote, PageHeader, Panel, RowMenu, type RowAction, SearchInput, Skeleton, Status, Table, Td, bytes, useRowMenu } from "../components/ui";

/** Shown by default: managed stores that run, are stopped or busy, and connected stores. Failed, errored, disabled and removed ones need the "Show disabled and failed" filter. */
export function isActive(s: ObjectStore): boolean {
  if (s.mode !== "managed") return true;
  return !["failed", "error", "missing", "disabled", "removed"].includes(s.status ?? "");
}

export function ModeBadge({ mode }: { mode: string }) {
  return mode === "managed" ? <Badge>Managed</Badge> : <Badge>Connected</Badge>;
}

/** The store is the App Store `forge-rustfs` container, taken over in place. */
export function AdoptedBadge() {
  return (
    <span title="Adopted from the App Store: Forge runs the existing forge-rustfs container and cannot reinstall it.">
      <Badge>Adopted</Badge>
    </span>
  );
}

/** Free of capacity summed over members that reported both; `—` otherwise. */
export function CapacityCell({ store }: { store: ObjectStore }) {
  const known = (store.members ?? []).filter((m) => typeof m.capacity_bytes === "number" && typeof m.free_bytes === "number" && (m.capacity_bytes ?? 0) > 0);
  if (!known.length) return <span className="text-xs text-faint">—</span>;
  const cap = known.reduce((a, m) => a + (m.capacity_bytes ?? 0), 0);
  const free = known.reduce((a, m) => a + (m.free_bytes ?? 0), 0);
  const used = Math.min(1, Math.max(0, (cap - free) / cap));
  const tone = used > 0.9 ? "bg-bad" : used > 0.75 ? "bg-warn" : "bg-faint";
  return (
    <div className="w-28" title={`${bytes(free)} free of ${bytes(cap)} (${Math.round(used * 100)}% used)`}>
      <div className="font-mono text-[11px] tabular-nums">
        {bytes(free)} <span className="text-faint">/ {bytes(cap)}</span>
      </div>
      <div className="mt-0.5 h-1 overflow-hidden rounded bg-panel-2">
        <div className={`h-full ${tone}`} style={{ width: `${used * 100}%` }} />
      </div>
    </div>
  );
}

export function Storage() {
  const stores = useObjectStores();
  const navigate = useNavigate();
  const search = useSearch({ from: "/storage" });
  const mode = search.mode ?? "all";
  const showAll = search.show === "all";
  const setSearch = (next: { mode?: "all" | "managed" | "external"; show?: boolean }) =>
    void navigate({
      to: "/storage",
      search: {
        mode: (next.mode ?? mode) === "all" ? undefined : ((next.mode ?? mode) as "managed" | "external"),
        show: (next.show ?? showAll) ? "all" : undefined,
      },
      replace: true,
    });
  const toast = useToast();
  const act = useStoreAction();
  const rm = useRowMenu();
  const [removing, setRemoving] = useState<ObjectStore | null>(null);
  const [disabling, setDisabling] = useState<ObjectStore | null>(null);
  const [disconnecting, setDisconnecting] = useState<ObjectStore | null>(null);
  const [assigning, setAssigning] = useState<ObjectStore | null>(null);
  const [q, setQ] = useState("");
  const [installing, setInstalling] = useState(false);
  const [connecting, setConnecting] = useState(false);
  const [adopting, setAdopting] = useState(false);

  const all = stores.data ?? [];
  const rows = useMemo(() => {
    const needle = q.trim().toLowerCase();
    return all.filter(
      (s) =>
        (mode === "all" || s.mode === mode) &&
        (showAll || isActive(s)) &&
        (!needle ||
          [s.name, s.slug, s.provider, s.endpoint, ...(s.members ?? []).map((m) => m.hostname)].some((v) => (v ?? "").toLowerCase().includes(needle))),
    );
  }, [all, q, mode, showAll]);
  const hidden = all.filter((s) => !isActive(s)).length;

  const run = (s: ObjectStore, action: "disable" | "enable" | "disconnect") =>
    act.mutate(
      { id: s.id, action },
      {
        onSuccess: () => {
          setDisabling(null);
          setDisconnecting(null);
          toast(action === "disconnect" ? `Disconnected ${s.name}` : `${action === "disable" ? "Disabling" : "Enabling"} ${s.name}`);
        },
        onError: (e) => toast(errorText(e), "error"),
      },
    );

  const rowActions = (s: ObjectStore): RowAction[] => {
    const busy = storeBusy(s);
    const busyHint = busy ? `Busy: ${s.status}` : undefined;
    // Reached through its environment only: nothing here to manage.
    if (s.limited) return [];
    const assign: RowAction = { label: "Change environment…", icon: Layers, onSelect: () => setAssigning(s) };
    if (s.mode !== "managed") return [assign, { label: "Disconnect", icon: Unplug, danger: true, onSelect: () => setDisconnecting(s) }];
    return [
      assign,
      { label: "Enable", icon: Play, hidden: s.status !== "disabled", disabled: busy, hint: busyHint, onSelect: () => run(s, "enable") },
      {
        label: "Disable",
        icon: Pause,
        hidden: s.status === "disabled",
        disabled: busy || isAdopted(s) || ["failed", "removed"].includes(s.status ?? ""),
        hint: busyHint ?? (isAdopted(s) ? "Not available for an adopted store" : "Nothing runs to disable"),
        onSelect: () => setDisabling(s),
      },
      { label: "Remove…", icon: Trash2, danger: true, disabled: busy, hint: busyHint, onSelect: () => setRemoving(s) },
    ];
  };

  const missing = stores.error instanceof ApiError && stores.error.status === 404;
  const actions = (
    <>
      <Button onClick={() => setAdopting(true)} title="Manage an App Store RustFS installation as an object store">
        <PackageCheck className="size-4" /> Adopt App Store RustFS
      </Button>
      <Button onClick={() => setConnecting(true)}>
        <Cable className="size-4" /> Connect existing
      </Button>
      <Button variant="primary" onClick={() => setInstalling(true)}>
        <Plus className="size-4" /> Install RustFS
      </Button>
    </>
  );

  return (
    <div>
      <PageHeader title="Object Storage" description="S3-compatible stores: RustFS that Forge installs and runs, and existing S3 services you connect." actions={missing ? undefined : actions} />
      <Panel
        actions={
          all.length > 0 ? (
            <div className="flex flex-wrap items-center gap-2">
              <div role="group" aria-label="Mode" className="flex rounded-md border border-line p-0.5 text-xs">
                {(["all", "managed", "external"] as const).map((m) => (
                  <button
                    key={m}
                    aria-pressed={mode === m}
                    onClick={() => setSearch({ mode: m })}
                    className={`rounded px-2 py-0.5 ${mode === m ? "bg-panel-2 text-fg" : "text-muted hover:text-fg"}`}
                  >
                    {m === "all" ? `All ${all.length}` : m === "managed" ? "Managed" : "Connected"}
                  </button>
                ))}
              </div>
              <label className="flex items-center gap-1.5 text-xs text-muted" title="Failed, errored, disabled and removed stores are hidden by default">
                <input type="checkbox" checked={showAll} onChange={(e) => setSearch({ show: e.target.checked })} />
                Show disabled and failed{hidden ? ` (${hidden})` : ""}
              </label>
              <SearchInput value={q} onChange={setQ} placeholder="Filter stores…" />
            </div>
          ) : undefined
        }
      >
        {stores.isLoading ? (
          <Skeleton rows={3} />
        ) : missing ? (
          <Empty icon={<Database className="size-5" />} title="Object storage is not available">
            This server does not offer object storage yet. Update Forge to use it.
          </Empty>
        ) : stores.error ? (
          <ErrorNote error={stores.error} />
        ) : !all.length ? (
          <Empty icon={<Database className="size-5" />} title="No object stores yet" action={<div className="flex flex-wrap justify-center gap-2">{actions}</div>}>
            Install RustFS on a node for a self-hosted S3 store, adopt a RustFS you installed from the App Store, or connect AWS S3, R2, MinIO or any S3-compatible service.
          </Empty>
        ) : !rows.length ? (
          <Empty title="No stores match">
            {!showAll && hidden ? `${hidden} disabled or failed store${hidden === 1 ? " is" : "s are"} hidden. Turn on "Show disabled and failed" to see them.` : `Clear the filter to see all ${all.length} stores.`}
          </Empty>
        ) : (
          <Table head={["Name", "Provider", "Environment", "Node", "Endpoint", "Release", "Status", "Capacity", "Last error", ""]}>
            {rows.map((s) => (
              <tr key={s.id} className="text-[13px] hover:bg-panel-2/50" {...(s.limited ? {} : rm.rowProps(rowActions(s), `Actions for ${s.name}`))}>
                <Td className="max-w-[14rem]">
                  <Link to="/storage/$storeId" params={{ storeId: s.id }} className="block truncate font-medium hover:text-accent" title={s.name}>
                    {s.name}
                  </Link>
                  {s.slug && <div className="truncate font-mono text-[11px] text-faint">{s.slug}</div>}
                </Td>
                <Td className="whitespace-nowrap">
                  <div>{providerLabel(s.provider)}</div>
                  <span className="flex flex-wrap gap-1">
                    <ModeBadge mode={s.mode} />
                    {isAdopted(s) && <AdoptedBadge />}
                  </span>
                </Td>
                <Td className="max-w-[14rem] text-xs">
                  {s.by_grant ? (
                    <span className="text-faint" title="You reach buckets of this store through a grant">
                      By grant
                    </span>
                  ) : (
                    <StoreOwner store={s} />
                  )}
                </Td>
                <Td className="max-w-[12rem]">
                  {s.limited ? (
                    <span className="block truncate" title={s.members.map((m) => m.hostname).join(", ")}>
                      {s.members.map((m) => m.hostname).filter(Boolean).join(", ") || <span className="text-xs text-faint">—</span>}
                    </span>
                  ) : s.members.length ? (
                    s.members.map((m) => (
                      <Link key={m.server_id} to="/nodes/$nodeId" params={{ nodeId: m.server_id }} className="block truncate hover:text-accent" title={[m.endpoint_host, ...m.data_paths].filter(Boolean).join(" · ")}>
                        {m.hostname || m.server_id.slice(0, 8)}
                      </Link>
                    ))
                  ) : (
                    <span className="text-xs text-faint">—</span>
                  )}
                </Td>
                <Td className="max-w-[16rem]">
                  <span className="block truncate font-mono text-[11px] select-all" title={s.endpoint ?? undefined}>
                    {s.endpoint || "—"}
                  </span>
                </Td>
                <Td className="font-mono text-[11px] whitespace-nowrap tabular-nums">
                  {s.mode === "managed" && !s.limited ? (
                    <span title={s.version && s.version !== s.deployed_version ? `requested ${s.version}` : undefined}>
                      {s.deployed_version || s.version || "—"}
                      {s.runtime && <span className="text-faint"> · {s.runtime}</span>}
                    </span>
                  ) : (
                    <span className="text-faint">—</span>
                  )}
                </Td>
                <Td>
                  <Status value={s.status} />
                </Td>
                <Td>
                  <CapacityCell store={s} />
                </Td>
                <Td className="max-w-[16rem]">
                  {s.last_error ? (
                    <span className="block truncate text-xs text-bad" title={s.last_error}>
                      {s.last_error}
                    </span>
                  ) : (
                    <span className="text-xs text-faint">—</span>
                  )}
                </Td>
                <Td className="w-10 text-right">
                  {!s.limited && <RowMenu actions={rowActions(s)} label={`Actions for ${s.name}`} />}
                </Td>
              </tr>
            ))}
          </Table>
        )}
        {rm.menu}
      </Panel>
      {!missing && <FoundOnNodes />}
      <InstallStoreDialog open={installing} onOpenChange={setInstalling} />
      <RemoveStoreDialog store={removing} onOpenChange={(o) => !o && setRemoving(null)} />
      <Confirm
        open={!!disabling}
        onOpenChange={(o) => !o && setDisabling(null)}
        title={`Disable ${disabling?.name ?? ""}?`}
        body={<p>Stops the store on every node. Data, settings and the credential stay. It is hidden from the default list until you select Enable.</p>}
        confirmLabel="Disable"
        busy={act.isPending}
        onConfirm={() => disabling && run(disabling, "disable")}
      />
      <Confirm
        open={!!disconnecting}
        onOpenChange={(o) => !o && setDisconnecting(null)}
        title={`Disconnect ${disconnecting?.name ?? ""}?`}
        body={<p>Forge forgets this connection and its credential. The S3 service and its data are not touched.</p>}
        confirmLabel="Disconnect"
        busy={act.isPending}
        onConfirm={() => disconnecting && run(disconnecting, "disconnect")}
      />
      {assigning && <ChangeEnvironmentDialog store={assigning} onClose={() => setAssigning(null)} />}
      <AdoptStoreDialog open={adopting} onOpenChange={setAdopting} />
      <ConnectStoreDialog open={connecting} onOpenChange={setConnecting} onDone={(s) => void navigate({ to: "/storage/$storeId", params: { storeId: s.id } })} />
    </div>
  );
}
