import { Link } from "@tanstack/react-router";
import { Loader2, Plus, X } from "lucide-react";
import { useEffect, useMemo, useState, type FormEvent } from "react";

import { useInstallations, useServers } from "../../api/hooks";
import { ADOPTED_DATA, isAdopted, useAdoptStore, useConnectStore, useInstallStore, useObjectStores, useStoreAction } from "../../api/storage";
import type { Job, ObjectStore, Server, StoreTopology } from "../../api/types";
import { errorText, useToast } from "../../app/toast";
import { NodeJobTabs } from "../JobLog";
import { JobDetail } from "../JobsDrawer";
import { Button, ErrorNote, Field, IconButton, Input, Modal, Select } from "../ui";
import { EnvironmentField, StoreOwner } from "./StoreOwner";

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

const isWindows = (s?: Pick<Server, "os"> | null) => (s?.os ?? "").toLowerCase().startsWith("win");
const isX64 = (s?: Pick<Server, "arch"> | null) => ["x86_64", "amd64", "x64"].includes((s?.arch ?? "").toLowerCase());
const nodeName = (s: Server) => s.display_name || s.hostname;

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

const SLUG = "[a-z][a-z0-9\\-]{0,30}";

// ------------------------------------------------------------------ install

/** RustFS erasure coding needs at least this many drives across the cluster. */
export const MIN_DISTRIBUTED_DRIVES = 4;
const MAX_DRIVES_PER_NODE = 16;
const MAX_NODES = 32;

interface MemberDraft {
  key: number;
  server_id: string;
  /** `null` = the default path derived from the instance name. */
  paths: (string | null)[];
  endpoint_host: string;
}

interface InstallForm {
  topology: StoreTopology;
  name: string;
  /** `null` = derived from the name. */
  slug: string | null;
  runtime: "native" | "docker";
  version: string;
  require_mount: boolean;
  s3_port: string;
  console_port: string;
  drives: number;
  members: MemberDraft[];
  /** The environment the store belongs to; "" for a shared store. */
  environment_id: string;
}

let memberSeq = 0;
const draft = (drives: number): MemberDraft => ({ key: ++memberSeq, server_id: "", paths: Array.from({ length: drives }, () => null), endpoint_host: "" });

const fresh = (): InstallForm => ({
  topology: "standalone",
  name: "",
  slug: null,
  runtime: "native",
  version: "latest",
  require_mount: false,
  s3_port: "9000",
  console_port: "",
  drives: 1,
  members: [draft(1)],
  environment_id: "",
});

function defaultPath(win: boolean, slug: string, drive: number, drives: number) {
  const s = slug || "<name>";
  if (win) return `D:\\ForgeData\\objstore\\${s}`;
  return drives > 1 ? `/srv/objstore/${s}/disk${drive + 1}` : `/srv/objstore/${s}`;
}

type Step = "topology" | "form" | "review" | "job";
const STEP_LABEL: Record<Step, string> = { topology: "Topology", form: "Nodes and drives", review: "Review", job: "Job" };

const TOPOLOGIES: { id: StoreTopology; label: string; text: string }[] = [
  { id: "standalone", label: "Standalone", text: "One node, one data path. No redundancy: the store is down while the node is down. Linux or Windows." },
  {
    id: "distributed",
    label: "Distributed (HA)",
    text: `An erasure-coded RustFS cluster across 2 or more Linux nodes, kept in sync by RustFS. Survives the loss of a node or drives. Needs at least ${MIN_DISTRIBUTED_DRIVES} drives in total.`,
  },
];

const th = "px-2 py-1.5 font-medium";

/** Install RustFS: topology → nodes and drives → review → job. */
export function InstallStoreDialog({ open, onOpenChange, onDone }: { open: boolean; onOpenChange: (o: boolean) => void; onDone?: (s?: ObjectStore) => void }) {
  const servers = useServers();
  const install = useInstallStore();
  const toast = useToast();
  const [f, setF] = useState<InstallForm>(fresh);
  const [step, setStep] = useState<Step>("topology");
  const [result, setResult] = useState<{ store?: ObjectStore; job?: Job; jobs?: Job[] }>();

  useEffect(() => {
    if (open) {
      setF(fresh());
      setStep("topology");
      setResult(undefined);
      install.reset();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const set = (p: Partial<InstallForm>) => setF((o) => ({ ...o, ...p }));
  const setMember = (key: number, p: Partial<MemberDraft>) => setF((o) => ({ ...o, members: o.members.map((m) => (m.key === key ? { ...m, ...p } : m)) }));

  const nodes = useMemo(() => (servers.data ?? []).filter((s) => s && s.status !== "revoked"), [servers.data]);
  const byId = (id: string) => nodes.find((n) => n.id === id);
  const distributed = f.topology === "distributed";
  const slug = f.slug ?? slugify(f.name);
  const lead = f.members[0];
  const first = byId(lead?.server_id ?? "");
  const win = !distributed && isWindows(first);
  const runtime = win ? "native" : f.runtime;

  const resolved = f.members.map((m) => {
    const node = byId(m.server_id);
    const w = isWindows(node);
    return {
      ...m,
      node,
      paths: m.paths.map((p, i) => (p ?? defaultPath(w, slug, i, m.paths.length)).trim()),
      host: m.endpoint_host.trim(),
    };
  });

  const s3 = Number(f.s3_port);
  const consolePort = f.console_port.trim() ? Number(f.console_port) : undefined;
  const portsOk = s3 >= 1024 && s3 <= 65535 && (consolePort === undefined || (consolePort >= 1024 && consolePort <= 65535 && consolePort !== s3));
  const chosen = resolved.filter((m) => m.node);
  const totalDrives = chosen.reduce((a, m) => a + m.paths.length, 0);
  const allPaths = resolved.flatMap((m) => m.paths.map((p) => `${m.server_id}|${p}`));

  const problems: string[] = [];
  if (!portsOk) problems.push("Ports must be 1024–65535 and differ from each other.");
  if (resolved.some((m) => !m.node)) problems.push(distributed ? "Choose a node for every row, or remove the row." : "Choose a node.");
  if (win && !isX64(first)) problems.push("RustFS on Windows needs an x86_64 node.");
  if (win && (lead?.paths.length ?? 1) !== 1) problems.push("Windows stores use exactly one data path.");
  if (new Set(allPaths).size !== allPaths.length) problems.push("Each data path must be unique on its node.");
  if (resolved.some((m) => m.paths.some((p) => !p))) problems.push("Every drive needs a data path.");
  if (distributed) {
    if (chosen.length < 2) problems.push("A distributed store needs at least 2 nodes.");
    if (f.members.length > MAX_NODES) problems.push(`A distributed store spans at most ${MAX_NODES} nodes.`);
    const hosts = chosen.map((m) => m.host.toLowerCase()).filter(Boolean);
    if (new Set(hosts).size !== hosts.length) problems.push("Endpoint hosts must be unique across nodes.");
    if (totalDrives < MIN_DISTRIBUTED_DRIVES) problems.push(`RustFS needs at least ${MIN_DISTRIBUTED_DRIVES} drives in total; this has ${totalDrives}.`);
  }

  const endpointOf = (host: string) => `http://${host || "<node address>"}:${f.s3_port || "9000"}`;

  const setDrives = (n: number) => {
    const d = Math.max(1, Math.min(MAX_DRIVES_PER_NODE, Math.floor(n) || 1));
    setF((o) => ({ ...o, drives: d, members: o.members.map((m) => ({ ...m, paths: Array.from({ length: d }, (_, i) => m.paths[i] ?? null) })) }));
  };

  const chooseTopology = (t: StoreTopology) =>
    setF((o) => {
      const drives = t === "distributed" ? Math.max(o.drives, 2) : 1;
      const head = o.members[0] ?? draft(drives);
      const keep = { ...head, paths: Array.from({ length: drives }, (_, i) => head.paths[i] ?? null) };
      const members = t === "distributed" ? [keep, ...(o.members.length > 1 ? o.members.slice(1) : [draft(drives)])] : [keep];
      return { ...o, topology: t, drives, members };
    });

  const submit = () => {
    const members = resolved.map((m) => ({ server_id: m.server_id, data_paths: m.paths, endpoint_host: m.endpoint_host.trim() || undefined }));
    install.mutate(
      {
        name: f.name.trim(),
        slug,
        provider: "rustfs",
        version: f.version.trim() || "latest",
        runtime,
        topology: f.topology,
        require_mount: f.require_mount,
        s3_port: s3,
        console_port: consolePort,
        members,
        environment_id: f.environment_id || undefined,
      },
      {
        onSuccess: (r) => {
          setResult(r ?? {});
          setStep("job");
          toast(`Installing ${f.name.trim()}`);
          onDone?.(r?.store);
        },
        onError: (e) => toast(errorText(e), "error"),
      },
    );
  };

  const nodeOptions = (key: number) => {
    const taken = new Set(f.members.filter((m) => m.key !== key).map((m) => m.server_id));
    return nodes.map((n) => {
      const why = n.status !== "online" ? n.status : distributed && isWindows(n) ? "Linux only" : taken.has(n.id) ? "already added" : "";
      return (
        <option key={n.id} value={n.id} disabled={!!why}>
          {nodeName(n)} · {n.os}/{n.arch}
          {why ? ` (${why})` : ""}
        </option>
      );
    });
  };

  const summary = `${chosen.length} node${chosen.length === 1 ? "" : "s"} × ${f.drives} drive${f.drives === 1 ? "" : "s"} = ${totalDrives} drives`;

  return (
    <Modal open={open} onOpenChange={onOpenChange} title="Install RustFS" description="A managed S3-compatible object store on your nodes." wide>
      <ol className="mb-4 flex flex-wrap gap-x-4 gap-y-1 border-b border-line pb-2 text-xs text-faint" aria-label="Steps">
        {(Object.keys(STEP_LABEL) as Step[]).map((s, i) => (
          <li key={s} aria-current={step === s ? "step" : undefined} className={step === s ? "font-medium text-fg" : ""}>
            {i + 1}. {STEP_LABEL[s]}
          </li>
        ))}
      </ol>

      {step === "topology" && (
        <div className="space-y-3">
          <div role="radiogroup" aria-label="Topology" className="divide-y divide-line rounded-md border border-line">
            {TOPOLOGIES.map((t) => (
              <label key={t.id} className={`flex cursor-pointer items-start gap-2.5 px-3 py-2.5 text-[13px] ${f.topology === t.id ? "bg-panel-2" : "hover:bg-panel-2/50"}`}>
                <input type="radio" name="topology" className="mt-0.5" checked={f.topology === t.id} onChange={() => chooseTopology(t.id)} />
                <span>
                  <span className="font-medium">{t.label}</span>
                  <span className="block text-xs text-muted">{t.text}</span>
                </span>
              </label>
            ))}
          </div>
          <div className="flex justify-end gap-2">
            <Button variant="ghost" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button variant="primary" onClick={() => setStep("form")}>
              Next
            </Button>
          </div>
        </div>
      )}

      {step === "form" && (
        <form
          className="space-y-4"
          onSubmit={(e: FormEvent) => {
            e.preventDefault();
            if (!problems.length) setStep("review");
          }}
        >
          <div className="grid gap-3 sm:grid-cols-3">
            <Field label="Name">
              <Input autoFocus required maxLength={64} value={f.name} onChange={(e) => set({ name: e.target.value })} placeholder="media" />
            </Field>
            <Field label="Instance name" hint="Service, container and folder name. Unique per node.">
              <Input
                required
                pattern={SLUG}
                className="font-mono"
                value={slug}
                onChange={(e) => set({ slug: e.target.value })}
                title="Lowercase letter first, then lowercase letters, digits or hyphens (max 31)"
              />
            </Field>
            <Field label="Release" hint="latest or exact, e.g. 1.0.5">
              <Input required className="font-mono" value={f.version} pattern="latest|[0-9A-Za-z.\-]+" onChange={(e) => set({ version: e.target.value })} />
            </Field>
            <Field label="Runtime" hint={win ? "Windows: native only (scheduled task)." : distributed && runtime === "docker" ? "Docker with host networking on every node." : "Native: systemd. Docker: rustfs/rustfs."}>
              <Select value={runtime} disabled={win} onChange={(e) => set({ runtime: e.target.value === "docker" ? "docker" : "native" })}>
                <option value="native">Native</option>
                <option value="docker">Docker</option>
              </Select>
            </Field>
            <Field label="S3 port" hint={distributed ? "Same on every node." : undefined}>
              <Input required type="number" min={1024} max={65535} className="font-mono" value={f.s3_port} onChange={(e) => set({ s3_port: e.target.value })} />
            </Field>
            <Field label="Console port" hint="Optional web console">
              <Input type="number" min={1024} max={65535} className="font-mono" value={f.console_port} placeholder="—" onChange={(e) => set({ console_port: e.target.value })} />
            </Field>
          </div>
          <EnvironmentField value={f.environment_id} onChange={(v) => set({ environment_id: v })} />

          {distributed ? (
            <section aria-label="Nodes and drives" className="space-y-2">
              <div className="flex flex-wrap items-center justify-between gap-2">
                <span className="text-xs font-medium text-muted">Nodes and drives</span>
                <label className="flex items-center gap-2 text-xs text-muted">
                  Drives per node
                  <Input type="number" min={1} max={MAX_DRIVES_PER_NODE} className="!w-16 font-mono" value={f.drives} onChange={(e) => setDrives(Number(e.target.value))} />
                </label>
              </div>
              <div className="overflow-x-auto rounded-md border border-line">
                <table className="w-full text-left text-[13px]">
                  <thead className="bg-panel-2 text-[11px] tracking-wide text-muted uppercase">
                    <tr>
                      <th className={th}>Node</th>
                      <th className={th}>Endpoint host</th>
                      <th className={th}>Data paths ({f.drives})</th>
                      <th className="w-8" />
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-line">
                    {f.members.map((m, mi) => {
                      const r = resolved[mi];
                      return (
                        <tr key={m.key} className="align-top">
                          <td className="w-48 min-w-[10rem] px-2 py-1.5">
                            <Select aria-label={`Node ${mi + 1}`} value={m.server_id} onChange={(e) => setMember(m.key, { server_id: e.target.value })}>
                              <option value="">{servers.isLoading ? "Loading…" : "Choose a node"}</option>
                              {nodeOptions(m.key)}
                            </Select>
                          </td>
                          <td className="w-44 min-w-[9rem] px-2 py-1.5">
                            <Input
                              aria-label={`Endpoint host for node ${mi + 1}`}
                              className="font-mono !text-xs"
                              value={m.endpoint_host}
                              placeholder="node address (automatic)"
                              onChange={(e) => setMember(m.key, { endpoint_host: e.target.value })}
                            />
                          </td>
                          <td className="min-w-[12rem] space-y-1 px-2 py-1.5">
                            {m.paths.map((p, i) => (
                              <Input
                                key={i}
                                aria-label={`Node ${mi + 1} drive ${i + 1} data path`}
                                className="font-mono !text-xs"
                                value={p ?? r?.paths[i] ?? ""}
                                onChange={(e) => setMember(m.key, { paths: m.paths.map((x, j) => (j === i ? e.target.value : x)) })}
                              />
                            ))}
                          </td>
                          <td className="px-1 py-1.5">
                            <IconButton type="button" label="Remove node" title={f.members.length <= 1 ? "The store needs at least one node" : "Remove node"} disabled={f.members.length <= 1} onClick={() => set({ members: f.members.filter((x) => x.key !== m.key) })}>
                              <X className="size-3.5" />
                            </IconButton>
                          </td>
                        </tr>
                      );
                    })}
                  </tbody>
                </table>
              </div>
              <div className="flex flex-wrap items-center justify-between gap-2">
                <Button type="button" variant="ghost" disabled={f.members.length >= MAX_NODES} title={f.members.length >= MAX_NODES ? `At most ${MAX_NODES} nodes` : undefined} onClick={() => set({ members: [...f.members, draft(f.drives)] })}>
                  <Plus className="size-4" /> Add node
                </Button>
                <span className={`font-mono text-[11px] tabular-nums ${totalDrives >= MIN_DISTRIBUTED_DRIVES && chosen.length >= 2 ? "text-muted" : "text-bad"}`}>
                  {summary} · minimum {MIN_DISTRIBUTED_DRIVES}
                </span>
              </div>
              <p className="text-xs text-faint">Linux nodes only. Every node gets the same number of drives, at least 4 in total. The node is the failure unit: drives of one node may share a disk (the job log notes it). Every node must reach every other on its endpoint host and the S3 port.</p>
            </section>
          ) : (
            lead && (
              <div className="grid gap-3 sm:grid-cols-2">
                <Field label="Node" hint={win ? "Windows support is implemented but not yet field-tested." : undefined}>
                  <Select required value={lead.server_id} onChange={(e) => setMember(lead.key, { server_id: e.target.value })} disabled={servers.isLoading}>
                    <option value="">{servers.isLoading ? "Loading nodes…" : "Choose a node"}</option>
                    {nodeOptions(lead.key)}
                  </Select>
                </Field>
                <Field label="Endpoint host" hint={`Reachable from the control plane: ${endpointOf(resolved[0]?.host ?? "")}`}>
                  <Input className="font-mono" value={lead.endpoint_host} placeholder="node address (automatic)" onChange={(e) => setMember(lead.key, { endpoint_host: e.target.value })} />
                </Field>
                <div className="sm:col-span-2">
                  <Field
                    label="Data path"
                    hint={
                      win
                        ? "Absolute, e.g. D:\\ForgeData\\objstore\\media. Not under \\Windows, \\Program Files, \\ProgramData or \\Users."
                        : "Absolute, e.g. /srv/objstore/media. Not a system folder (/, /etc, /usr, /tmp, /home…). Never deleted on uninstall."
                    }
                  >
                    <Input required className="font-mono" value={lead.paths[0] ?? resolved[0]?.paths[0] ?? ""} onChange={(e) => setMember(lead.key, { paths: [e.target.value] })} />
                  </Field>
                </div>
              </div>
            )
          )}

          <label className="flex items-start gap-2 text-[13px]">
            <input type="checkbox" className="mt-0.5" checked={f.require_mount} onChange={(e) => set({ require_mount: e.target.checked })} />
            <span>
              Require a dedicated mount
              <span className="block text-xs text-faint">Refuse to install or start unless every data path is its own mount point, so data never lands on the root disk.</span>
            </span>
          </label>
          {problems.length > 0 && (f.name || f.members.some((m) => m.server_id)) && (
            <ul className="space-y-0.5 text-xs text-bad">
              {problems.map((p) => (
                <li key={p}>{p}</li>
              ))}
            </ul>
          )}
          <div className="flex justify-end gap-2">
            <Button type="button" variant="ghost" onClick={() => setStep("topology")}>
              Back
            </Button>
            <Button variant="primary" disabled={problems.length > 0}>
              Review
            </Button>
          </div>
        </form>
      )}

      {step === "review" && (
        <div className="space-y-3">
          <dl className="grid grid-cols-[7rem_minmax(0,1fr)] gap-x-3 gap-y-1.5 text-[13px]">
            <dt className="text-muted">Topology</dt>
            <dd>
              {distributed ? "Distributed (HA)" : "Standalone"}
              {distributed && <span className="ml-2 font-mono text-[11px] text-muted tabular-nums">{summary}</span>}
            </dd>
            <dt className="text-muted">Runtime</dt>
            <dd>
              {runtime === "docker" ? "Docker" : "Native"} <span className="ml-1 font-mono text-[11px] text-muted">RustFS {f.version || "latest"}</span>
            </dd>
            <dt className="text-muted">Instance</dt>
            <dd className="font-mono text-xs">forge-objstore-{slug}</dd>
            <dt className="text-muted">Ports</dt>
            <dd className="font-mono text-xs tabular-nums">
              S3 {s3}
              {consolePort ? ` · console ${consolePort}` : ""}
            </dd>
            <dt className="text-muted">Mount</dt>
            <dd className="text-xs">{f.require_mount ? "Dedicated mount required for every data path" : "Not required"}</dd>
            <dt className="text-muted">Environment</dt>
            <dd className="min-w-0 text-xs">
              <StoreOwner store={{ environment_id: f.environment_id || null }} />
            </dd>
          </dl>
          <div className="overflow-x-auto rounded-md border border-line">
            <table className="w-full text-left text-[13px]">
              <thead className="bg-panel-2 text-[11px] tracking-wide text-muted uppercase">
                <tr>
                  <th className={th}>Node</th>
                  <th className={th}>Data path</th>
                  <th className={th}>Endpoint</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-line">
                {resolved.map((m) => (
                  <tr key={m.key} className="align-top">
                    <td className="px-2 py-1.5 whitespace-nowrap">
                      {m.node ? nodeName(m.node) : "—"} <span className="font-mono text-[11px] text-faint">{m.node ? `${m.node.os}/${m.node.arch}` : ""}</span>
                    </td>
                    <td className="px-2 py-1.5 font-mono text-[11px] break-all">
                      {m.paths.map((p, i) => (
                        <div key={i}>{p}</div>
                      ))}
                    </td>
                    <td className="px-2 py-1.5 font-mono text-[11px] break-all">{endpointOf(m.host)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <div className="text-xs text-muted">
            <div className="mb-1 font-medium text-fg">What happens</div>
            <ol className="list-decimal space-y-0.5 pl-4">
              <li>A root credential is generated and stored in the vault as “{f.name.trim()} (object store root)”.</li>
              <li>
                {runtime === "docker"
                  ? `rustfs/rustfs:${f.version || "latest"} is pulled and started as a container`
                  : win
                    ? "rustfs.exe is downloaded, checksum-verified and registered as a SYSTEM scheduled task"
                    : "The release is downloaded, checksum-verified and installed as a systemd service"}
                {distributed ? ` on each of the ${chosen.length} nodes, configured with all ${totalDrives} drives.` : "."}
              </li>
              <li>The firewall opens the S3{consolePort ? " and console ports" : " port"}. Data paths are created if missing and never deleted on uninstall.</li>
              <li>Forge waits for the health check{distributed ? " on every node" : ""}, then marks the store running.</li>
              {distributed && <li>If drives of one node share a disk, the node's job log warns: the node is the failure unit. Forge cannot see the disks before the install.</li>}
              <li>If the first install fails on every node, Forge removes what it set up and the store record; the job log stays in Jobs.</li>
            </ol>
          </div>
          {distributed && (
            <p className="rounded-md border border-line px-3 py-2 text-xs text-muted">
              Members must reach each other on{" "}
              <span className="font-mono text-[11px] text-fg">{chosen.map((m) => `${m.host}:${s3}`).join(", ") || `<endpoint_host>:${s3}`}</span>; RustFS starts once a quorum of members is
              up (install can take several minutes).
            </p>
          )}
          <ErrorNote error={install.error} />
          <div className="flex justify-end gap-2">
            <Button variant="ghost" onClick={() => setStep("form")} disabled={install.isPending}>
              Back
            </Button>
            <Button variant="primary" onClick={submit} disabled={install.isPending}>
              {install.isPending && <Loader2 className="size-4 animate-spin" />}
              Install
            </Button>
          </div>
        </div>
      )}

      {step === "job" && (
        <div className="space-y-3">
          <InstallJobs result={result} nodeLabel={(id) => (byId(id) ? nodeName(byId(id) as Server) : id.slice(0, 8))} />
          <div className="flex justify-end gap-2">
            {result?.store?.id && (
              <Link to="/storage/$storeId" params={{ storeId: result.store.id }} onClick={() => onOpenChange(false)}>
                <Button>Open store</Button>
              </Link>
            )}
            <Button variant="primary" onClick={() => onOpenChange(false)}>
              Close
            </Button>
          </div>
        </div>
      )}
    </Modal>
  );
}

/** One install job per member (distributed), with per-node tabs; a single job shows its full detail. */
function InstallJobs({ result, nodeLabel }: { result?: { job?: Job; jobs?: Job[] }; nodeLabel: (serverId: string) => string }) {
  const jobs = (Array.isArray(result?.jobs) && result.jobs.length ? result.jobs : result?.job ? [result.job] : []).filter((j) => j?.id);
  if (!jobs.length) return <p className="text-muted">Install submitted.</p>;
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
export function AdoptStoreDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (o: boolean) => void }) {
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

  useEffect(() => {
    if (open) {
      setServerId("");
      setName("");
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

  const chosen = candidates.find((c) => c.inst.server_id === serverId);
  const port = chosen?.inst.params?.port || "9000";
  const endpoint = `http://${host.trim() || "<node address>"}:${port}`;
  const loading = servers.isLoading || installations.isLoading;

  return (
    <Modal open={open} onOpenChange={onOpenChange} title="Adopt App Store RustFS" description="Manage an App Store RustFS installation as an object store." wide>
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
              { name: name.trim(), server_id: serverId, endpoint_host: host.trim() || undefined, environment_id: environmentId || undefined },
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
          <div className="text-xs text-muted">
            <div className="mb-1 font-medium text-fg">What happens</div>
            <ol className="list-decimal space-y-0.5 pl-4">
              <li>Forge reads the container's key on the node and seals it in the Credential vault as the store's root credential.</li>
              <li>The container keeps running; nothing is pulled, recreated or moved.</li>
              <li>The App Store entry for RustFS on that node is removed. Start, stop, restart and uninstall now happen on the store page.</li>
              <li>Repair is not available: Forge cannot reinstall a container it did not create.</li>
            </ol>
          </div>
          <ErrorNote error={adopt.error} />
          <div className="flex justify-end gap-2">
            <Button type="button" variant="ghost" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button variant="primary" disabled={adopt.isPending || !serverId || !name.trim() || !!chosen?.why}>
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
