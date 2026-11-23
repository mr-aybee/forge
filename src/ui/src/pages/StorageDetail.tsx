import { Link, useNavigate, useParams } from "@tanstack/react-router";
import { Activity, ArrowLeft, Database, HardDrive, KeyRound, Layers, Loader2, Play, Plus, RefreshCw, RotateCw, Square, Trash2, Unplug, Wrench } from "lucide-react";
import { useState, type ReactNode } from "react";

import { ApiError } from "../api/client";
import { isAdopted, keysSupported, liveMembers, runningMembers, storeBusy, useBuckets, useCreateBucket, useDeleteBucket, useMyBucketGrants, useObjectStore, useStoreAction, useTestStore, type StoreAction } from "../api/storage";
import type { ObjectStore } from "../api/types";
import { errorText, useToast } from "../app/toast";
import { JobDetail } from "../components/JobsDrawer";
import { AccessKeys } from "../components/storage/AccessKeys";
import { BucketGrants, grantAccessLabel } from "../components/storage/BucketGrants";
import { ObjectExplorer } from "../components/storage/ObjectExplorer";
import { providerLabel } from "../components/storage/StoreDialogs";
import { ChangeEnvironmentDialog, StoreOwner } from "../components/storage/StoreOwner";
import { type ActionTone, Badge, Button, actionHover, Confirm, CopyButton, Drawer, Empty, ErrorNote, Field, IconButton, Input, Modal, Panel, Skeleton, Status, Table, Td, ago, time, toMs } from "../components/ui";
import { AdoptedBadge, CapacityCell, ModeBadge } from "./Storage";

const LIFECYCLE: { action: StoreAction; label: string; icon: typeof Play; hint: string; tone?: ActionTone }[] = [
  { action: "start", label: "Start", icon: Play, tone: "ok", hint: "Start the service" },
  { action: "stop", label: "Stop", icon: Square, tone: "bad", hint: "Stop the service; S3 clients lose access until it starts again" },
  { action: "restart", label: "Restart", icon: RotateCw, tone: "warn", hint: "Restart the service; expect a short outage" },
  { action: "status", label: "Refresh status", icon: RefreshCw, hint: "Check the service, health and capacity on the node" },
  { action: "install", label: "Repair", icon: Wrench, hint: "Re-run the install: binaries, unit, firewall. Data and credential are kept" },
];

const IMPACT: Partial<Record<StoreAction, string>> = {
  stop: "S3 clients and apps using this store lose access until it is started again.",
  restart: "S3 requests fail for a few seconds while the service restarts.",
  install: "Re-runs the installer for the requested release. Data and the root credential are kept; expect a short outage.",
};

export function StorageDetail() {
  const { storeId } = useParams({ from: "/storage/$storeId" });
  const store = useObjectStore(storeId);
  const s = store.data;

  if (store.isLoading) return <Skeleton rows={6} />;
  if (store.error || !s)
    return (
      <div className="space-y-3">
        <Back />
        {store.error instanceof ApiError && store.error.status === 404 ? (
          <Empty icon={<Database className="size-5" />} title="Store not found" action={<Link to="/storage"><Button>All stores</Button></Link>}>
            It may have been uninstalled or disconnected.
          </Empty>
        ) : (
          <ErrorNote error={store.error ?? "Store unavailable"} />
        )}
      </div>
    );
  return <Detail s={s} />;
}

function Back() {
  return (
    <Link to="/storage" className="inline-flex items-center gap-1 text-xs text-muted hover:text-accent">
      <ArrowLeft className="size-3.5" /> Object Storage
    </Link>
  );
}

function Detail({ s }: { s: ObjectStore }) {
  const managed = s.mode === "managed";
  const busy = storeBusy(s);
  const toast = useToast();
  const navigate = useNavigate();
  const act = useStoreAction();
  const test = useTestStore();
  const [pending, setPending] = useState<StoreAction | null>(null);
  const [confirm, setConfirm] = useState<StoreAction | null>(null);
  const [jobId, setJobId] = useState<string | null>(null);
  const [bucket, setBucket] = useState<string | null>(null);
  const [assigning, setAssigning] = useState(false);
  // Through bucket grants only: what the person may do differs per bucket.
  const mine = useMyBucketGrants(s.by_grant);
  const held = (mine.data ?? []).filter((g) => g.store_id === s.id);
  const grant = held.find((g) => g.bucket === bucket);

  const run = (action: StoreAction) => {
    setPending(action);
    act.mutate(
      { id: s.id, action },
      {
        onSuccess: (r) => {
          setConfirm(null);
          if (action === "disconnect") {
            toast(`Disconnected ${s.name}`);
            void navigate({ to: "/storage" });
            return;
          }
          toast(`${action === "status" ? "Status check" : action === "install" ? "Repair" : action.charAt(0).toUpperCase() + action.slice(1)} queued for ${s.name}`);
          const j = r?.jobs?.[0]?.id;
          if (j) setJobId(j);
        },
        onError: (e) => toast(errorText(e), "error"),
        onSettled: () => setPending(null),
      },
    );
  };

  const nodes = s.members.map((m) => m.hostname || m.server_id.slice(0, 8)).join(", ") || "its node";
  const busyWhy = busy ? `Busy: ${s.status}. Wait for the current job to finish.` : undefined;
  const adopted = isAdopted(s);
  const adoptedWhy = "Not available for an adopted store: Forge keeps the App Store forge-rustfs container as it is and does not reinstall it.";
  const keysWhy = busyWhy ?? (s.status === "running" || s.status === "degraded" ? undefined : `The store is ${s.status}. Start it to manage access keys.`);

  return (
    <div className="space-y-4">
      <Back />
      <header className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-2">
            <h1 className="truncate text-lg font-semibold tracking-tight" title={s.name}>
              {s.name}
            </h1>
            <ModeBadge mode={s.mode} />
            {adopted && <AdoptedBadge />}
            <Status value={s.status} />
            {managed && !s.limited && <ClusterHealth s={s} />}
          </div>
          <div className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted">
            <span>{providerLabel(s.provider)}</span>
            {!s.by_grant && (
              <span className="inline-flex min-w-0 max-w-[20rem] items-center gap-1">
                Environment <span className="min-w-0 text-fg"><StoreOwner store={s} /></span>
              </span>
            )}
            {managed && <span>{topologyLabel(s)}</span>}
            {s.endpoint && (
              <span className="inline-flex min-w-0 items-center gap-1">
                <span className="truncate font-mono text-[11px] text-fg select-all" title={s.endpoint}>
                  {s.endpoint}
                </span>
              </span>
            )}
            {s.region && <span className="font-mono text-[11px]">{s.region}</span>}
            {s.path_style !== undefined && <span>{s.path_style ? "path-style" : "virtual-host"}</span>}
            {managed && !s.limited && (
              <span className="font-mono text-[11px] tabular-nums" title={s.version && s.version !== s.deployed_version ? `requested ${s.version}` : undefined}>
                RustFS {s.deployed_version || s.version || "—"} · {s.runtime ?? "—"}
                {s.s3_port ? ` · :${s.s3_port}` : ""}
                {s.console_port ? ` · console :${s.console_port}` : ""}
              </span>
            )}
            {s.slug && <span className="font-mono text-[11px] text-faint">{s.slug}</span>}
            {s.credential_id && (
              <Link to="/vault" className="hover:text-accent" title="Credential in the vault">
                credential ↗
              </Link>
            )}
          </div>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          {s.endpoint && <CopyButton text={s.endpoint} label="Endpoint" />}
          {!s.limited && (
            <Button title="Assign this store to an environment, or make it shared" onClick={() => setAssigning(true)}>
              <Layers className="size-4" /> Change environment
            </Button>
          )}
          {!s.by_grant && (
            <Button
              disabled={test.isPending}
              onClick={() =>
                test.mutate(s.id, {
                  onSuccess: (r) =>
                    r?.ok
                      ? toast(`Reachable in ${r.latency_ms ?? "?"} ms${typeof r.buckets === "number" ? ` · ${r.buckets} bucket${r.buckets === 1 ? "" : "s"}` : ""}`)
                      : toast(`Not reachable: ${r?.error ?? "unknown error"}`, "error"),
                  onError: (e) => toast(errorText(e), "error"),
                })
              }
            >
              {test.isPending ? <Loader2 className="size-4 animate-spin" /> : <Activity className="size-4" />}
              Test connection
            </Button>
          )}
        </div>
      </header>

      {s.last_error && (
        <div className="flex items-start gap-2 rounded-md border border-bad/40 bg-bad-soft px-3 py-2 text-[13px] text-bad">
          <span className="min-w-0 flex-1 break-words">{s.last_error}</span>
          {s.last_job_id && (
            <button className="shrink-0 text-xs underline" onClick={() => setJobId(s.last_job_id ?? null)}>
              Last job
            </button>
          )}
        </div>
      )}

      {s.by_grant ? (
        <div className="space-y-2">
          <div className="flex flex-wrap items-center gap-x-4 gap-y-1 rounded-md border border-line px-3 py-2 text-xs text-muted">
            <span>You have access to buckets of this store through a grant. You work with their objects here, in the API and in the CLI; no S3 key is issued, and the store's operators run it.</span>
            {held.map((g) => (
              <span key={g.id} className="inline-flex items-center gap-1.5" title={g.expires_at ? `Until ${time(g.expires_at)}` : "This grant has no end"}>
                <span className="font-mono text-[11px] text-fg">{g.bucket}</span>
                {grantAccessLabel(g.access)}
                {g.expires_at ? ` · ends ${ago(g.expires_at)}` : ""}
              </span>
            ))}
          </div>
          {mine.error ? <ErrorNote error={mine.error} /> : null}
        </div>
      ) : s.limited ? (
        <div className="flex flex-wrap items-center gap-x-4 gap-y-1 rounded-md border border-line px-3 py-2 text-xs text-muted">
          <span>This store belongs to your environment. You can browse its buckets and attach them to the environment's services; its operators run it.</span>
          {s.members.map((m, i) => (
            <span key={`${m.hostname ?? ""}-${i}`} className="inline-flex items-center gap-1.5">
              <span className="text-fg">{m.hostname || "node"}</span>
              {m.status && <Status value={m.status} />}
            </span>
          ))}
        </div>
      ) : managed ? (
        <Panel
          title="Members"
          description={
            adopted
              ? "The App Store forge-rustfs container, adopted in place. Uninstall never deletes data."
              : s.topology === "distributed"
                ? "RustFS keeps the members in sync (erasure coding). Uninstall never deletes data."
                : "A node with an exclusive data path. Uninstall never deletes data."
          }
          actions={
            <div className="flex flex-wrap gap-1.5">
              {LIFECYCLE.map((a) => (
                <Button
                  key={a.action}
                  className={a.tone ? actionHover[a.tone] : undefined}
                  title={a.action === "install" && adopted ? adoptedWhy : (busyWhy ?? a.hint)}
                  disabled={busy || act.isPending || (a.action === "install" && adopted)}
                  onClick={() => (IMPACT[a.action] ? setConfirm(a.action) : run(a.action))}
                >
                  {pending === a.action ? <Loader2 className="size-4 animate-spin" /> : <a.icon className="size-4" />}
                  {a.label}
                </Button>
              ))}
              <Button variant="danger" title={busyWhy ?? (adopted ? "Remove the forge-rustfs container; data and credential are kept" : "Remove the service; data and credential are kept")} disabled={busy || act.isPending} onClick={() => setConfirm("uninstall")}>
                {pending === "uninstall" ? <Loader2 className="size-4 animate-spin" /> : <Trash2 className="size-4" />}
                Uninstall
              </Button>
              {s.last_job_id && !s.last_error && (
                <Button variant="ghost" onClick={() => setJobId(s.last_job_id ?? null)}>
                  Last job
                </Button>
              )}
            </div>
          }
        >
          {s.members.length ? (
            <Table head={["Node", "Endpoint host", `Drives`, "Mount", "Capacity", "Status", "Observed"]}>
              {s.members.map((m) => {
                const obs = toMs(m.observed_at);
                return (
                  <tr key={m.server_id} className={`text-[13px] ${m.status === "removed" ? "opacity-60" : ""}`}>
                    <Td className="align-top">
                      <Link to="/nodes/$nodeId" params={{ nodeId: m.server_id }} className="hover:text-accent">
                        {m.hostname || m.server_id.slice(0, 8)}
                      </Link>
                    </Td>
                    <Td className="max-w-[12rem] align-top">
                      <span className="block truncate font-mono text-[11px] select-all" title={m.endpoint_host ?? undefined}>
                        {m.endpoint_host || "—"}
                      </span>
                    </Td>
                    <Td className="max-w-[18rem] align-top">
                      {m.data_paths.length ? (
                        <ul title={`${m.data_paths.length} drive${m.data_paths.length === 1 ? "" : "s"}`}>
                          {m.data_paths.map((p) => (
                            <li key={p} className="truncate font-mono text-[11px] select-all" title={p}>
                              {p}
                            </li>
                          ))}
                        </ul>
                      ) : (
                        <span className="text-xs text-faint">—</span>
                      )}
                    </Td>
                    <Td className="whitespace-nowrap">
                      {m.dedicated === true ? (
                        <Badge tone="ok">dedicated</Badge>
                      ) : m.dedicated === false ? (
                        <span title="The data path shares a filesystem with other data">
                          <Badge tone={m.require_mount ? "bad" : "warn"}>shared disk</Badge>
                        </span>
                      ) : (
                        <span className="text-xs text-faint">—</span>
                      )}
                      {m.require_mount && <span className="ml-1 text-[11px] text-faint">required</span>}
                    </Td>
                    <Td>
                      <CapacityCell store={{ ...s, members: [m] }} />
                    </Td>
                    <Td>{m.status ? <Status value={m.status} /> : <span className="text-xs text-faint">—</span>}</Td>
                    <Td className="text-xs whitespace-nowrap text-muted" title={obs ? time(obs) : undefined}>
                      {obs ? ago(obs) : "—"}
                    </Td>
                  </tr>
                );
              })}
            </Table>
          ) : (
            <Empty title="No members">This store has no node assigned.</Empty>
          )}
        </Panel>
      ) : (
        <div className="flex items-center justify-between gap-3 rounded-md border border-line px-3 py-2 text-xs text-muted">
          <span>Connected store: Forge browses its buckets and objects but does not run or update it.</span>
          <Button variant="danger" disabled={act.isPending} onClick={() => setConfirm("disconnect")}>
            {pending === "disconnect" ? <Loader2 className="size-4 animate-spin" /> : <Unplug className="size-4" />}
            Disconnect
          </Button>
        </div>
      )}

      <Panel>
        <div className="grid min-h-[22rem] md:grid-cols-[14rem_minmax(0,1fr)]">
          <Buckets storeId={s.id} selected={bucket} onSelect={setBucket} disabled={busy || s.status === "stopped"} granted={s.by_grant} />
          <div className="min-w-0 border-t border-line md:border-t-0 md:border-l">
            {bucket ? (
              <ObjectExplorer key={bucket} storeId={s.id} bucket={bucket} readOnly={s.by_grant && grant?.access !== "readwrite"} />
            ) : (
              <Empty icon={<HardDrive className="size-5" />} title="Choose a bucket">
                {s.by_grant ? "Pick a bucket on the left to browse its objects." : "Pick a bucket on the left to browse, upload and download objects."}
              </Empty>
            )}
          </div>
        </div>
      </Panel>

      {s.by_grant ? null : s.limited ? (
        <p className="flex items-center gap-2 rounded-md border border-line px-3 py-2 text-xs text-muted">
          <KeyRound className="size-3.5 shrink-0" />
          <span className="min-w-0">To give a service a key, attach a bucket on the service's Storage tab. Standalone keys are issued by the store's operators.</span>
        </p>
      ) : keysSupported(s) ? (
        <AccessKeys s={s} disabled={keysWhy} />
      ) : (
        <p className="flex items-center gap-2 rounded-md border border-line px-3 py-2 text-xs text-muted">
          <KeyRound className="size-3.5 shrink-0" />
          <span className="min-w-0">Create keys for a connected store at its provider.</span>
        </p>
      )}

      {!s.limited && <BucketGrants s={s} />}

      {assigning && <ChangeEnvironmentDialog store={s} onClose={() => setAssigning(false)} />}
      <Confirm
        open={!!confirm}
        onOpenChange={(o) => !o && setConfirm(null)}
        title={
          confirm === "uninstall"
            ? `Uninstall ${s.name}?`
            : confirm === "disconnect"
              ? `Disconnect ${s.name}?`
              : `${LIFECYCLE.find((a) => a.action === confirm)?.label ?? ""} ${s.name}?`
        }
        body={<ConfirmBody action={confirm} s={s} nodes={nodes} />}
        confirmLabel={confirm === "uninstall" ? "Uninstall" : confirm === "disconnect" ? "Disconnect" : (LIFECYCLE.find((a) => a.action === confirm)?.label ?? "Continue")}
        busy={act.isPending}
        onConfirm={() => confirm && run(confirm)}
      />
      <Drawer open={!!jobId} onOpenChange={(o) => !o && setJobId(null)} title={`${s.name}: job`} wide>
        {jobId && <JobDetail id={jobId} />}
      </Drawer>
    </div>
  );
}

function ConfirmBody({ action, s, nodes }: { action: StoreAction | null; s: ObjectStore; nodes: string }): ReactNode {
  if (action === "uninstall" && isAdopted(s))
    return (
      <div className="space-y-2">
        <p>
          Stops and removes the adopted <span className="font-mono text-xs">forge-rustfs</span> container on {nodes}.
        </p>
        <p className="text-fg">
          Data is kept: <span className="font-mono text-xs">{s.members.flatMap((m) => m.data_paths).join(", ") || "the data path"}</span> is not deleted, and the root credential stays in the Credential vault for recovery.
        </p>
        <p>Apps and clients using {s.endpoint ?? "this endpoint"}, including keys issued for it, lose access.</p>
      </div>
    );
  if (action === "uninstall")
    return (
      <div className="space-y-2">
        <p>
          Stops and removes RustFS (<span className="font-mono text-xs">forge-objstore-{s.slug ?? "?"}</span>) on {nodes}, and closes its firewall ports.
        </p>
        <p className="text-fg">
          Data is kept: {s.members.flatMap((m) => m.data_paths).join(", ") || "the data paths"} {s.members.flatMap((m) => m.data_paths).length > 1 ? "are" : "is"} not deleted, and the root credential stays in the vault for recovery.
        </p>
        <p>Apps and clients using {s.endpoint ?? "this endpoint"} lose access.</p>
      </div>
    );
  if (action === "disconnect")
    return (
      <p>
        Forge forgets this store and deletes its stored access key. Buckets and objects at <span className="font-mono text-xs">{s.endpoint ?? "the endpoint"}</span> are not touched.
      </p>
    );
  return (
    <p>
      {IMPACT[action ?? "status"] ?? ""} Runs on {nodes}.
    </p>
  );
}

/** The buckets of a store. With `granted` (reached through bucket grants only) the list is the granted buckets, without create and delete. */
function Buckets({ storeId, selected, onSelect, disabled, granted }: { storeId: string; selected: string | null; onSelect: (b: string | null) => void; disabled: boolean; granted: boolean }) {
  const buckets = useBuckets(storeId);
  const create = useCreateBucket();
  const remove = useDeleteBucket();
  const toast = useToast();
  const [adding, setAdding] = useState(false);
  const [name, setName] = useState("");
  const [del, setDel] = useState<string | null>(null);
  const list = buckets.data ?? [];

  return (
    <aside className="min-w-0" aria-label="Buckets">
      <div className="flex items-center justify-between border-b border-line px-3 py-2">
        <span className="text-[11px] font-semibold tracking-wider text-faint uppercase">Buckets {list.length ? list.length : ""}</span>
        <span className="flex">
          <IconButton label="Refresh buckets" className="!size-7" onClick={() => void buckets.refetch()}>
            <RefreshCw className={`size-3.5 ${buckets.isFetching ? "animate-spin" : ""}`} />
          </IconButton>
          {!granted && (
            <IconButton label="Create bucket" className="!size-7" onClick={() => setAdding(true)} disabled={disabled} title={disabled ? "The store is not running" : "Create bucket"}>
              <Plus className="size-3.5" />
            </IconButton>
          )}
        </span>
      </div>
      {buckets.isLoading ? (
        <Skeleton rows={3} />
      ) : buckets.error ? (
        <ErrorNote error={buckets.error} />
      ) : !list.length && granted ? (
        <div className="p-3 text-xs text-muted">None of the buckets you were granted exists any more.</div>
      ) : !list.length ? (
        <div className="p-3 text-xs text-muted">
          No buckets.{" "}
          <button className="text-accent underline disabled:opacity-50" disabled={disabled} onClick={() => setAdding(true)}>
            Create one
          </button>
        </div>
      ) : (
        <ul className="max-h-[32rem] overflow-y-auto py-1">
          {list.map((b) => {
            const ms = toMs(b.created ?? undefined);
            return (
              <li key={b.name} className="group flex items-center">
                <button
                  onClick={() => onSelect(b.name)}
                  aria-current={selected === b.name}
                  title={ms ? `${b.name} · created ${time(ms)}` : b.name}
                  className={`min-w-0 flex-1 truncate px-3 py-1.5 text-left font-mono text-xs ${selected === b.name ? "bg-accent-soft text-accent" : "hover:bg-panel-2"}`}
                >
                  {b.name}
                </button>
                {!granted && (
                  <IconButton label={`Delete bucket ${b.name}`} className="!size-7 opacity-0 group-hover:opacity-100 focus:opacity-100 hover:!text-bad" onClick={() => setDel(b.name)}>
                    <Trash2 className="size-3.5" />
                  </IconButton>
                )}
              </li>
            );
          })}
        </ul>
      )}

      <Modal open={adding} onOpenChange={(o) => (setAdding(o), o || setName(""))} title="Create bucket">
        <form
          className="space-y-3"
          onSubmit={(e) => {
            e.preventDefault();
            create.mutate(
              { id: storeId, name: name.trim() },
              {
                onSuccess: () => {
                  toast(`Created bucket ${name.trim()}`);
                  onSelect(name.trim());
                  setAdding(false);
                  setName("");
                },
                onError: (err) => toast(errorText(err), "error"),
              },
            );
          }}
        >
          <Field label="Name" hint="3–63 lowercase letters, digits, dots and hyphens; starts and ends with a letter or digit.">
            <Input autoFocus required className="font-mono" minLength={3} maxLength={63} pattern="[a-z0-9][a-z0-9.\-]{1,61}[a-z0-9]" value={name} onChange={(e) => setName(e.target.value)} />
          </Field>
          <div className="flex justify-end gap-2">
            <Button type="button" variant="ghost" onClick={() => setAdding(false)}>
              Cancel
            </Button>
            <Button variant="primary" disabled={create.isPending}>
              {create.isPending && <Loader2 className="size-4 animate-spin" />}
              Create
            </Button>
          </div>
        </form>
      </Modal>
      <Confirm
        open={!!del}
        onOpenChange={(o) => !o && setDel(null)}
        title={`Delete bucket ${del ?? ""}?`}
        body="Only empty buckets can be deleted. Apps that write to this bucket start failing. This needs the storage.manage permission and an MFA-verified sign-in."
        confirmLabel="Delete bucket"
        busy={remove.isPending}
        onConfirm={() =>
          del &&
          remove.mutate(
            { id: storeId, bucket: del },
            {
              onSuccess: () => {
                toast(`Deleted bucket ${del}`);
                if (selected === del) onSelect(null);
                setDel(null);
              },
              onError: (e) => toast(e instanceof ApiError && e.status === 409 ? `${del} is not empty; delete its objects first` : errorText(e), "error"),
            },
          )
        }
      />
    </aside>
  );
}

function topologyLabel(s: ObjectStore) {
  const n = liveMembers(s).length;
  return s.topology === "distributed" ? `Distributed · ${n} node${n === 1 ? "" : "s"}` : "Standalone";
}

/** `<running>/<members excluding removed> members online`; the store status carries `degraded`. */
function ClusterHealth({ s }: { s: ObjectStore }) {
  const live = liveMembers(s);
  if (!live.length) return null;
  const up = runningMembers(s).length;
  const down = live.filter((m) => m.status !== "running").map((m) => `${m.hostname || m.server_id.slice(0, 8)} (${m.status ?? "unknown"})`);
  return (
    <span className={`font-mono text-[11px] tabular-nums ${up < live.length ? "text-warn" : "text-muted"}`} title={down.length ? `Not running: ${down.join(", ")}` : "All members running"}>
      {up}/{live.length} members online
    </span>
  );
}
