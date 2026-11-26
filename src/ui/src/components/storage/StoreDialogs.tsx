import { Link } from "@tanstack/react-router";
import { Loader2 } from "lucide-react";
import { useEffect, useMemo, useState, type FormEvent } from "react";

import { useInstallations, useServers } from "../../api/hooks";
import { ADOPTED_DATA, isAdopted, useAdoptStore, useConnectStore, useObjectStores, useStoreAction } from "../../api/storage";
import type { DiscoveredContainer, Job, ObjectStore, Server } from "../../api/types";
import { errorText, useToast } from "../../app/toast";
import { NodeJobTabs } from "../JobLog";
import { JobDetail } from "../JobsDrawer";
import { Button, ErrorNote, Field, Input, Modal, Select } from "../ui";
import { EnvironmentField } from "./StoreOwner";

export const PROVIDERS: { id: string; label: string }[] = [
  { id: "aws", label: "AWS S3" },
  { id: "s3", label: "S3-compatible" },
  { id: "rustfs", label: "RustFS" },
  { id: "minio", label: "MinIO" },
  { id: "ceph", label: "Ceph" },
  { id: "r2", label: "Cloudflare R2" },
  { id: "wasabi", label: "Wasabi" },
  { id: "b2", label: "Backblaze B2" },
  { id: "other", label: "Other" },
];

export const providerLabel = (p?: string | null) => PROVIDERS.find((x) => x.id === p)?.label ?? p ?? "—";

export const isWindows = (s?: Pick<Server, "os"> | null) => (s?.os ?? "").toLowerCase().startsWith("win");
export const isX64 = (s?: Pick<Server, "arch"> | null) => ["x86_64", "amd64", "x64"].includes((s?.arch ?? "").toLowerCase());
export const nodeName = (s: Server) => s.display_name || s.hostname;

/** `My Store 1` → `my-store-1` (slug rule: [a-z][a-z0-9-]{0,30}). */
export function slugify(name: string): string {
  let s = name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^[^a-z]+/, "")
    .replace(/-+$/, "");
  s = s.slice(0, 31).replace(/-+$/, "");
  return s || "objstore";
}

export const SLUG = "[a-z][a-z0-9\\-]{0,30}";

// ------------------------------------------------------------------ install
// The install form itself is a page: `pages/StorageInstall.tsx` (`/storage/install`).

/** RustFS erasure coding needs at least this many drives across the cluster. */
export const MIN_DISTRIBUTED_DRIVES = 4;

/** Why Docker cannot be used on a node, or null. `unknown` (older server, failed scan) does not block. */
export function dockerBlocked(state?: string): string | null {
  if (state === "not_installed") return "Docker not installed";
  if (state === "unreachable") return "Docker unreachable";
  if (state === "unsupported_os") return "Docker not supported on this OS";
  return null;
}

/** One install job per member (distributed), with per-node tabs; a single job shows its full detail. */
export function InstallJobs({ result, nodeLabel, submitted = "Install submitted." }: { result?: { job?: Job; jobs?: Job[] }; nodeLabel: (serverId: string) => string; submitted?: string }) {
  const jobs = (Array.isArray(result?.jobs) && result.jobs.length ? result.jobs : result?.job ? [result.job] : []).filter((j) => j?.id);
  if (!jobs.length) return <p className="text-muted">{submitted}</p>;
  if (jobs.length === 1 && jobs[0]) return <JobDetail id={jobs[0].id} />;
  return (
    <NodeJobTabs
      jobs={jobs.map((j, i) => {
        const sid = typeof j.payload?.server_id === "string" ? j.payload.server_id : `job-${i}`;
        return { nodeId: sid, nodeName: sid.startsWith("job-") ? `Job ${i + 1}` : nodeLabel(sid), jobId: j.id };
      })}
    />
  );
}

// ------------------------------------------------------------------ adopt

/** Take over the App Store `rustfs` container on a Linux node as a managed store, without recreating it. */
export function AdoptStoreDialog({ open, onOpenChange, container }: { open: boolean; onOpenChange: (o: boolean) => void; container?: DiscoveredContainer | null }) {
  const servers = useServers();
  const installations = useInstallations();
  const stores = useObjectStores();
  const adopt = useAdoptStore();
  const toast = useToast();
  const [serverId, setServerId] = useState("");
  const [name, setName] = useState("");
  const [host, setHost] = useState("");
  const [environmentId, setEnvironmentId] = useState("");
  const [result, setResult] = useState<{ store?: ObjectStore; job?: Job } | null>(null);
  const [s3Port, setS3Port] = useState("");
  const [consolePort, setConsolePort] = useState("");

  useEffect(() => {
    if (open) {
      setServerId(container?.server_id ?? "");
      setName(container ? (container.forge_label ?? container.name.replace(/^forge-objstore-/, "")) : "");
      setS3Port("");
      setConsolePort("");
      setHost("");
      setEnvironmentId("");
      setResult(null);
      adopt.reset();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const adopted = useMemo(
    () => new Set((stores.data ?? []).filter(isAdopted).flatMap((s) => s.members.filter((m) => m.data_paths[0] === ADOPTED_DATA).map((m) => m.server_id))),
    [stores.data],
  );
  const candidates = useMemo(() => {
    const byId = new Map((Array.isArray(servers.data) ? servers.data : []).filter(Boolean).map((s) => [s.id, s]));
    const list = Array.isArray(installations.data) ? installations.data : [];
    return list
      .filter((i) => i?.package === "rustfs" && i.server_id)
      .map((i) => {
        const node = byId.get(i.server_id);
        const why = !node
          ? "node not found"
          : isWindows(node)
            ? "Linux only"
            : adopted.has(i.server_id)
              ? "already adopted"
              : node.status !== "online"
                ? node.status
                : "";
        return { inst: i, node, why };
      });
  }, [servers.data, installations.data, adopted]);

  const chosen = container ? undefined : candidates.find((c) => c.inst.server_id === serverId);
  const published = container?.ports.map((p) => /:(\d+)->/.exec(p)?.[1]).filter(Boolean) ?? [];
  const hostNet = container?.network === "host" || (!!container && !published.length);
  const port = container ? s3Port || published[0] || "9000" : chosen?.inst.params?.port || "9000";
  const portBad = [s3Port, consolePort].some((p) => p.trim() && !(Number(p) >= 1 && Number(p) <= 65535 && Number.isInteger(Number(p))));
  const endpoint = `http://${host.trim() || "<node address>"}:${port}`;
  const loading = servers.isLoading || installations.isLoading;

  return (
    <Modal
      open={open}
      onOpenChange={onOpenChange}
      title={container ? `Adopt ${container.name}` : "Adopt App Store RustFS"}
      description={container ? `Manage this RustFS container on ${container.hostname} as an object store.` : "Manage an App Store RustFS installation as an object store."}
      wide
    >
      {result ? (
        <div className="space-y-3">
          {result.job?.id ? <JobDetail id={result.job.id} /> : <p className="text-muted">Adoption submitted.</p>}
          <div className="flex justify-end gap-2">
            {result.store?.id && (
              <Link to="/storage/$storeId" params={{ storeId: result.store.id }} onClick={() => onOpenChange(false)}>
                <Button>Open store</Button>
              </Link>
            )}
            <Button variant="primary" onClick={() => onOpenChange(false)}>
              Close
            </Button>
          </div>
        </div>
      ) : (
        <form
          className="space-y-3"
          onSubmit={(e) => {
            e.preventDefault();
            adopt.mutate(
              {
                name: name.trim(),
                server_id: serverId,
                endpoint_host: host.trim() || undefined,
                environment_id: environmentId || undefined,
                container: container?.name || undefined,
                s3_port: container && s3Port.trim() ? Number(s3Port) : undefined,
                console_port: container && consolePort.trim() ? Number(consolePort) : undefined,
              },
              {
                onSuccess: (r) => {
                  setResult(r ?? {});
                  toast(`Adopting RustFS as ${name.trim()}`);
                },
                onError: (err) => toast(errorText(err), "error"),
              },
            );
          }}
        >
          {container ? (
            <>
              <dl className="grid grid-cols-[6rem_minmax(0,1fr)] gap-x-3 gap-y-1 text-[13px]">
                <dt className="text-muted">Node</dt>
                <dd className="truncate">
                  <Link to="/nodes/$nodeId" params={{ nodeId: container.server_id }} className="hover:text-accent" onClick={() => onOpenChange(false)}>
                    {container.hostname}
                  </Link>
                </dd>
                <dt className="text-muted">Container</dt>
                <dd className="truncate font-mono text-xs select-all" title={container.id}>
                  {container.name} <span className="text-faint">{container.id.slice(0, 12)}</span>
                </dd>
                <dt className="text-muted">Image</dt>
                <dd className="truncate font-mono text-xs" title={container.image}>
                  {container.image || "—"}
                </dd>
                <dt className="text-muted">Ports</dt>
                <dd className="truncate font-mono text-[11px] tabular-nums" title={container.ports.join(", ")}>
                  {container.ports.join(", ") || (container.network === "host" ? "host network" : "—")}
                </dd>
              </dl>
              <div className="grid gap-3 sm:grid-cols-2">
                <Field label="Name">
                  <Input required autoFocus maxLength={64} value={name} onChange={(e) => setName(e.target.value)} placeholder="media" />
                </Field>
                <Field label="Endpoint host (optional)" hint={`Reachable from the control plane: ${endpoint}`}>
                  <Input className="font-mono" value={host} placeholder="node address (automatic)" onChange={(e) => setHost(e.target.value)} />
                </Field>
                <Field label={`S3 port${hostNet ? "" : " (optional)"}`} hint={hostNet ? "Host network: Forge cannot read the ports; enter them if they are not 9000/9001." : "Override only if the published port is wrong."}>
                  <Input type="number" min={1} max={65535} className="font-mono" value={s3Port} placeholder={published[0] ?? "9000"} onChange={(e) => setS3Port(e.target.value)} />
                </Field>
                <Field label="Console port (optional)">
                  <Input type="number" min={1} max={65535} className="font-mono" value={consolePort} placeholder={published[1] ?? "—"} onChange={(e) => setConsolePort(e.target.value)} />
                </Field>
                <div className="sm:col-span-2">
                  <EnvironmentField value={environmentId} onChange={setEnvironmentId} />
                </div>
              </div>
            </>
          ) : (
          <>
          <p className="rounded-md border border-line px-3 py-2 text-xs text-muted">
            Forge takes over the running <span className="font-mono text-[11px] text-fg">forge-rustfs</span> container without recreating it. Its data and key stay; the key moves into the Credential vault, and the
            App Store stops managing the container.
          </p>
          {installations.error ? (
            <ErrorNote error={installations.error} />
          ) : !loading && !candidates.length ? (
            <p className="px-1 py-2 text-xs text-muted">
              No node has an App Store RustFS installation.{" "}
              For a new store use <span className="font-medium text-fg">Install RustFS</span> instead.{" "}
              <Link to="/app-store/$packageId" params={{ packageId: "rustfs" }} className="text-accent hover:underline" onClick={() => onOpenChange(false)}>
                RustFS in the App Store ↗
              </Link>
            </p>
          ) : (
            <div className="grid gap-3 sm:grid-cols-2">
              <Field label="Node" hint={chosen ? `RustFS ${chosen.inst.version || "—"} · ${chosen.inst.status || "unknown"} · S3 port ${port}` : "Linux nodes with the App Store rustfs package."}>
                <Select required value={serverId} disabled={loading} onChange={(e) => setServerId(e.target.value)}>
                  <option value="">{loading ? "Loading…" : "Choose a node"}</option>
                  {candidates.map((c) => (
                    <option key={c.inst.id} value={c.inst.server_id} disabled={!!c.why}>
                      {c.node ? nodeName(c.node) : c.inst.server_id.slice(0, 8)} · RustFS {c.inst.version || "—"}
                      {c.why ? ` (${c.why})` : ""}
                    </option>
                  ))}
                </Select>
              </Field>
              <Field label="Name">
                <Input required maxLength={64} value={name} onChange={(e) => setName(e.target.value)} placeholder="media" />
              </Field>
              <div className="sm:col-span-2">
                <Field label="Endpoint host (optional)" hint={`Reachable from the control plane: ${endpoint}`}>
                  <Input className="font-mono" value={host} placeholder="node address (automatic)" onChange={(e) => setHost(e.target.value)} />
                </Field>
              </div>
              <div className="sm:col-span-2">
                <EnvironmentField value={environmentId} onChange={setEnvironmentId} />
              </div>
            </div>
          )}
          </>
          )}
          <div className="text-xs text-muted">
            <div className="mb-1 font-medium text-fg">What happens</div>
            <ol className="list-decimal space-y-0.5 pl-4">
              <li>Forge reads the container's key on the node and seals it in the Credential vault as the store's root credential.</li>
              <li>The container keeps running; nothing is pulled, recreated or moved.</li>
              {container ? (
                <li>Start, stop, restart and uninstall now happen on the store page.</li>
              ) : (
                <li>The App Store entry for RustFS on that node is removed. Start, stop, restart and uninstall now happen on the store page.</li>
              )}
              <li>Repair is not available: Forge cannot reinstall a container it did not create.</li>
            </ol>
          </div>
          <ErrorNote error={adopt.error} />
          <div className="flex justify-end gap-2">
            <Button type="button" variant="ghost" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button variant="primary" disabled={adopt.isPending || !serverId || !name.trim() || !!chosen?.why || portBad}>
              {adopt.isPending && <Loader2 className="size-4 animate-spin" />}
              Adopt
            </Button>
          </div>
        </form>
      )}
    </Modal>
  );
}

// ------------------------------------------------------------------ connect

const awsEndpoint = (region: string) => `https://s3.${region || "us-east-1"}.amazonaws.com`;

/** Connect an existing S3 endpoint. The secret is sent once and never shown again. */
export function ConnectStoreDialog({ open, onOpenChange, onDone }: { open: boolean; onOpenChange: (o: boolean) => void; onDone?: (s: ObjectStore) => void }) {
  const connect = useConnectStore();
  const toast = useToast();
  const [name, setName] = useState("");
  const [provider, setProvider] = useState("s3");
  const [region, setRegion] = useState("us-east-1");
  const [endpoint, setEndpoint] = useState("");
  const [endpointTouched, setEndpointTouched] = useState(false);
  const [pathStyle, setPathStyle] = useState(true);
  const [pathTouched, setPathTouched] = useState(false);
  const [accessKey, setAccessKey] = useState("");
  const [secretKey, setSecretKey] = useState("");
  const [bucket, setBucket] = useState("");
  const [environmentId, setEnvironmentId] = useState("");

  useEffect(() => {
    if (open) {
      setEnvironmentId("");
      setName("");
      setProvider("s3");
      setRegion("us-east-1");
      setEndpoint("");
      setEndpointTouched(false);
      setPathStyle(true);
      setPathTouched(false);
      setAccessKey("");
      setSecretKey("");
      setBucket("");
      connect.reset();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const aws = provider === "aws";
  const shownEndpoint = endpointTouched ? endpoint : aws ? awsEndpoint(region) : endpoint;
  const shownPathStyle = pathTouched ? pathStyle : !aws;

  const close = (o: boolean) => {
    if (!o) setSecretKey("");
    onOpenChange(o);
  };

  return (
    <Modal open={open} onOpenChange={close} title="Connect existing storage" description="Browse buckets on AWS S3 or any S3-compatible service. Forge checks the key before saving." wide>
      <form
        className="space-y-3"
        autoComplete="off"
        onSubmit={(e) => {
          e.preventDefault();
          connect.mutate(
            {
              name: name.trim(),
              provider,
              endpoint: shownEndpoint.trim(),
              region: region.trim() || "us-east-1",
              path_style: shownPathStyle,
              access_key: accessKey.trim(),
              secret_key: secretKey,
              bucket: bucket.trim() || undefined,
              environment_id: environmentId || undefined,
            },
            {
              onSuccess: (s) => {
                setSecretKey("");
                toast(`Connected ${s?.name ?? name.trim()}`);
                onOpenChange(false);
                if (s?.id) onDone?.(s);
              },
              onError: (e) => toast(errorText(e), "error"),
            },
          );
        }}
      >
        <div className="grid gap-3 sm:grid-cols-2">
          <Field label="Name">
            <Input autoFocus required maxLength={64} value={name} onChange={(e) => setName(e.target.value)} placeholder="backups-eu" />
          </Field>
          <Field label="Provider">
            <Select value={provider} onChange={(e) => setProvider(e.target.value)}>
              {PROVIDERS.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.label}
                </option>
              ))}
            </Select>
          </Field>
          <Field label="Endpoint" hint="http(s)://host[:port]">
            <Input
              required
              type="url"
              pattern="https?://.+"
              className="font-mono"
              value={shownEndpoint}
              placeholder="https://s3.example.com"
              onChange={(e) => {
                setEndpoint(e.target.value);
                setEndpointTouched(true);
              }}
            />
          </Field>
          <Field label="Region" hint={provider === "r2" ? "R2 uses auto." : undefined}>
            <Input className="font-mono" value={region} onChange={(e) => setRegion(e.target.value)} placeholder="us-east-1" />
          </Field>
          <Field label="Access key">
            <Input required className="font-mono" autoComplete="off" spellCheck={false} value={accessKey} onChange={(e) => setAccessKey(e.target.value)} />
          </Field>
          <Field label="Secret key" hint="Sealed in the vault; never shown again.">
            <Input required type="password" autoComplete="new-password" value={secretKey} onChange={(e) => setSecretKey(e.target.value)} />
          </Field>
          <Field label="Bucket (optional)" hint="For keys scoped to one bucket that cannot list all buckets.">
            <Input className="font-mono" value={bucket} onChange={(e) => setBucket(e.target.value)} placeholder="—" />
          </Field>
          <label className="flex items-start gap-2 self-center text-[13px]">
            <input
              type="checkbox"
              className="mt-0.5"
              checked={shownPathStyle}
              onChange={(e) => {
                setPathStyle(e.target.checked);
                setPathTouched(true);
              }}
            />
            <span>
              Path-style addressing
              <span className="block text-xs text-faint">endpoint/bucket/key. Most self-hosted servers need it; AWS uses virtual hosts.</span>
            </span>
          </label>
        </div>
        <EnvironmentField value={environmentId} onChange={setEnvironmentId} />
        <ErrorNote error={connect.error} />
        <div className="flex justify-end gap-2 pt-1">
          <Button type="button" variant="ghost" onClick={() => close(false)}>
            Cancel
          </Button>
          <Button variant="primary" disabled={connect.isPending}>
            {connect.isPending && <Loader2 className="size-4 animate-spin" />}
            Check and connect
          </Button>
        </div>
      </form>
    </Modal>
  );
}

/**
 * Remove a managed store: uninstall on every node (best effort; works for
 * failed and errored stores too) and keep or delete the data. Deleting data
 * needs the store's name typed and an MFA-verified session.
 */
export function RemoveStoreDialog({ store, onOpenChange }: { store: ObjectStore | null; onOpenChange: (o: boolean) => void }) {
  const act = useStoreAction();
  const toast = useToast();
  const [deleteData, setDeleteData] = useState(false);
  const [typed, setTyped] = useState("");
  useEffect(() => {
    setDeleteData(false);
    setTyped("");
  }, [store?.id]);
  if (!store) return null;
  const paths = store.members.flatMap((m) => m.data_paths);
  const nodes = store.members.map((m) => m.hostname || m.server_id.slice(0, 8)).join(", ") || "its nodes";
  const adopted = isAdopted(store);
  const canConfirm = !deleteData || typed.trim() === store.name;
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (!canConfirm) return;
    act.mutate(
      { id: store.id, action: "uninstall", delete_data: deleteData || undefined, confirm: deleteData ? typed.trim() : undefined },
      {
        onSuccess: (r) => {
          const skipped = r?.skipped?.length ? ` ${r.skipped.length} node(s) skipped: gone or revoked.` : "";
          toast(`Removing ${store.name} on ${r?.jobs?.length ?? 0} node(s).${skipped} See Jobs for each node's result.`);
          onOpenChange(false);
        },
        onError: (err) => toast(errorText(err), "error"),
      },
    );
  };
  return (
    <Modal open onOpenChange={onOpenChange} title={`Remove ${store.name}?`}>
      <form onSubmit={submit} className="space-y-3 text-muted">
        <p>
          Stops and removes {adopted ? "the adopted forge-rustfs container" : "RustFS"} on {nodes} and closes its firewall ports. Each node runs its own job; a node that fails is reported in Jobs and the store stays listed until every node is removed.
        </p>
        <fieldset className="space-y-2">
          <label className="flex items-start gap-2">
            <input type="radio" name="data" className="mt-0.5" checked={!deleteData} onChange={() => setDeleteData(false)} />
            <span>
              <span className="text-fg">Keep data</span> — {paths.join(", ") || "the data paths"} and the root credential in the vault stay for recovery.
            </span>
          </label>
          <label className="flex items-start gap-2">
            <input type="radio" name="data" className="mt-0.5" checked={deleteData} onChange={() => setDeleteData(true)} />
            <span>
              <span className="text-bad">Delete data</span> — every object in {paths.join(", ") || "the data paths"} on every node is deleted. This cannot be undone.
            </span>
          </label>
        </fieldset>
        {deleteData && (
          <Field label={`Type ${store.name} to confirm`}>
            <Input value={typed} onChange={(e) => setTyped(e.target.value)} autoFocus aria-label="Store name" />
          </Field>
        )}
        {act.error && <ErrorNote error={act.error} />}
        <div className="flex justify-end gap-2 pt-1">
          <Button type="button" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button type="submit" variant="danger" disabled={!canConfirm || act.isPending}>
            {act.isPending && <Loader2 className="size-4 animate-spin" />} {deleteData ? "Remove and delete data" : "Remove"}
          </Button>
        </div>
      </form>
    </Modal>
  );
}
