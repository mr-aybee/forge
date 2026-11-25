// Install RustFS as a full page (`/storage/install`): topology, basics, runtime, nodes and drives,
// network and Docker options, with a live preview of what will be created.
import { Link, useNavigate } from "@tanstack/react-router";
import { Box, Container, Cpu, Database, FolderTree, HardDrive, Layers, Loader2, Network, Plus, Server as ServerIcon, Settings2, Tag, X } from "lucide-react";
import { useMemo, useState } from "react";

import { useServers } from "../api/hooks";
import { useDockerOnNodes, useInstallStore } from "../api/storage";
import type { DockerVolume, Job, ObjectStore, Server, StoreTopology } from "../api/types";
import { errorText, useToast } from "../app/toast";
import { ChoiceCards, FieldError, FormPage, FormSection, type FormSectionMeta, type SectionState, SummaryPanel, SummaryRow } from "../components/FormPage";
import { DockerFields, type DockerDraft, type Kv, KvRows, REGION, VOLUME_NAME, dockerBody, dockerDraft, dockerProblems, optProblems } from "../components/storage/DockerOptions";
import { InstallJobs, MIN_DISTRIBUTED_DRIVES, dockerBlocked, isWindows, isX64, nodeName, slugify } from "../components/storage/StoreDialogs";
import { EnvironmentField, StoreOwner } from "../components/storage/StoreOwner";
import { Button, CopyButton, ErrorNote, Field, IconButton, Input, Select } from "../components/ui";

const MAX_DRIVES_PER_NODE = 16;
const MAX_NODES = 32;
const SLUG_RE = /^[a-z][a-z0-9-]{0,30}$/;
const VERSION_RE = /^(latest|[0-9A-Za-z.-]+)$/;

interface MemberDraft {
  key: number;
  server_id: string;
  /** `null` = the default path derived from the instance name. */
  paths: (string | null)[];
  endpoint_host: string;
}

interface VolumeDraft {
  kind: "bind" | "named";
  name: string;
  driver: string;
  options: Kv[];
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
  region: string;
  console: boolean;
  docker: DockerDraft;
  /** Per drive index, the same on every node. */
  volumes: VolumeDraft[];
}

const bindVolume = (): VolumeDraft => ({ kind: "bind", name: "", driver: "", options: [] });
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
  region: "us-east-1",
  console: false,
  docker: dockerDraft(),
  volumes: [bindVolume()],
});

function defaultPath(win: boolean, slug: string, drive: number, drives: number) {
  const s = slug || "<name>";
  if (win) return `D:\\ForgeData\\objstore\\${s}`;
  return drives > 1 ? `/srv/objstore/${s}/disk${drive + 1}` : `/srv/objstore/${s}`;
}

const portOk = (n: number) => Number.isInteger(n) && n >= 1024 && n <= 65535;
const bad = (e?: string) => (e ? "!border-bad" : "");
const th = "px-2 py-1.5 font-medium";

type Errors = Partial<Record<"name" | "slug" | "version" | "runtime" | "nodes" | "s3_port" | "console_port" | "region" | "docker" | "volumes", string[]>>;

export function StorageInstall() {
  const servers = useServers();
  const install = useInstallStore();
  const toast = useToast();
  const navigate = useNavigate();
  const [f, setF] = useState<InstallForm>(fresh);
  const [tried, setTried] = useState(false);
  const [result, setResult] = useState<{ store?: ObjectStore; job?: Job; jobs?: Job[] }>();

  const set = (p: Partial<InstallForm>) => setF((o) => ({ ...o, ...p }));
  const setMember = (key: number, p: Partial<MemberDraft>) => setF((o) => ({ ...o, members: o.members.map((m) => (m.key === key ? { ...m, ...p } : m)) }));

  const nodes = useMemo(() => (servers.data ?? []).filter((s) => s && s.status !== "revoked"), [servers.data]);
  const byId = (id: string) => nodes.find((n) => n.id === id);
  const distributed = f.topology === "distributed";
  const slug = f.slug ?? slugify(f.name);
  const lead = f.members[0];
  const first = byId(lead?.server_id ?? "");
  const win = !distributed && isWindows(first);
  const dockerState = useDockerOnNodes(
    f.members.map((m) => m.server_id),
    !result,
  );
  const dockerWhy = f.members
    .map((m) => {
      const why = dockerBlocked(dockerState[m.server_id]?.docker);
      const n = byId(m.server_id);
      return why && n ? `${why} on ${nodeName(n)}` : null;
    })
    .filter(Boolean)
    .join("; ");
  const dockerChecking = f.members.some((m) => m.server_id && dockerState[m.server_id]?.docker === "checking");
  const runtime = win ? "native" : f.runtime;
  const isDocker = runtime === "docker";

  const resolved = f.members.map((m) => {
    const node = byId(m.server_id);
    const w = isWindows(node);
    return { ...m, node, paths: m.paths.map((p, i) => (p ?? defaultPath(w, slug, i, m.paths.length)).trim()), host: m.endpoint_host.trim() };
  });

  const s3 = Number(f.s3_port);
  const consolePort = f.console && f.console_port.trim() ? Number(f.console_port) : undefined;
  const chosen = resolved.filter((m) => m.node);
  const totalDrives = chosen.reduce((a, m) => a + m.paths.length, 0);
  const allPaths = resolved.flatMap((m) => m.paths.map((p) => `${m.server_id}|${p}`));

  // Validation, per field so each message sits beside its input.
  const err: Errors = {};
  const add = (k: keyof Errors, msg: string) => (err[k] = [...(err[k] ?? []), msg]);
  if (!f.name.trim()) add("name", "Enter a name.");
  if (!SLUG_RE.test(slug)) add("slug", "Lowercase letter first, then lowercase letters, digits or hyphens (max 31).");
  if (!VERSION_RE.test(f.version.trim())) add("version", "latest or an exact version, e.g. 1.0.5.");
  if (!portOk(s3)) add("s3_port", "A port from 1024 to 65535.");
  if (f.console && !f.console_port.trim()) add("console_port", "Enter a console port, or turn the console off.");
  else if (consolePort !== undefined && (!portOk(consolePort) || consolePort === s3)) add("console_port", "1024–65535, different from the S3 port.");
  if (!REGION.test(f.region.trim())) add("region", "Lowercase letters, digits and hyphens, e.g. us-east-1.");
  if (isDocker && dockerWhy) add("runtime", `${dockerWhy}. Install Docker on the node or choose Native.`);
  if (isDocker) {
    dockerProblems(f.docker, { containerName: true }).forEach((p) => add("docker", p));
    f.volumes.slice(0, f.drives).forEach((v, i) => {
      if (v.kind !== "named") return;
      if (!VOLUME_NAME.test(v.name.trim())) add("volumes", `Drive ${i + 1}: volume name is 1–64 letters, digits, _ . - (letter or digit first).`);
      if (v.driver.trim() && !/^[a-zA-Z0-9][a-zA-Z0-9_.:/-]{0,127}$/.test(v.driver.trim())) add("volumes", `Drive ${i + 1}: driver name is not valid.`);
      optProblems(v.options, `Drive ${i + 1} driver options`).forEach((p) => add("volumes", p));
    });
    const names = f.volumes.slice(0, f.drives).filter((v) => v.kind === "named").map((v) => v.name.trim());
    if (new Set(names).size !== names.length) add("volumes", "Each named volume must be different.");
  }
  if (resolved.some((m) => !m.node)) add("nodes", distributed ? "Choose a node for every row, or remove the row." : "Choose a node.");
  if (win && !isX64(first)) add("nodes", "RustFS on Windows needs an x86_64 node.");
  if (win && (lead?.paths.length ?? 1) !== 1) add("nodes", "Windows stores use exactly one data path.");
  if (new Set(allPaths).size !== allPaths.length) add("nodes", "Each data path must be unique on its node.");
  if (resolved.some((m) => m.paths.some((p) => !p))) add("nodes", "Every drive needs a data path.");
  if (distributed) {
    if (chosen.length < 2) add("nodes", "A distributed store needs at least 2 nodes.");
    if (f.members.length > MAX_NODES) add("nodes", `A distributed store spans at most ${MAX_NODES} nodes.`);
    const hosts = chosen.map((m) => m.host.toLowerCase()).filter(Boolean);
    if (new Set(hosts).size !== hosts.length) add("nodes", "Endpoint hosts must be unique across nodes.");
    if (totalDrives < MIN_DISTRIBUTED_DRIVES) add("nodes", `RustFS needs at least ${MIN_DISTRIBUTED_DRIVES} drives in total; this has ${totalDrives}.`);
  }
  const problemCount = Object.values(err).reduce((a, l) => a + (l?.length ?? 0), 0);

  // A message shows once the person touched that part or pressed Install.
  const touched = { name: tried || !!f.name, nodes: tried || f.members.some((m) => m.server_id) };
  const show = (k: keyof Errors, gate = tried): string | undefined => (gate || tried ? err[k]?.join(" ") : undefined);
  const always = (k: keyof Errors) => err[k]?.join(" ");

  const state = (keys: (keyof Errors)[], done: boolean, gate = tried): SectionState => (keys.some((k) => err[k]?.length) && (gate || tried) ? "error" : done && !keys.some((k) => err[k]?.length) ? "ok" : "idle");

  const endpointOf = (host: string) => `http://${host || "<node address>"}:${f.s3_port || "9000"}`;
  const instance = (isDocker && f.docker.container_name.trim()) || `forge-objstore-${slug}`;
  const image = `${f.docker.image.trim() || "rustfs/rustfs"}:${f.version.trim() || "latest"}`;

  const setDrives = (n: number) => {
    const d = Math.max(1, Math.min(MAX_DRIVES_PER_NODE, Math.floor(n) || 1));
    setF((o) => ({ ...o, drives: d, volumes: Array.from({ length: d }, (_, i) => o.volumes[i] ?? bindVolume()), members: o.members.map((m) => ({ ...m, paths: Array.from({ length: d }, (_, i) => m.paths[i] ?? null) })) }));
  };

  const chooseTopology = (t: StoreTopology) =>
    setF((o) => {
      const drives = t === "distributed" ? Math.max(o.drives, 2) : 1;
      const head = o.members[0] ?? draft(drives);
      const keep = { ...head, paths: Array.from({ length: drives }, (_, i) => head.paths[i] ?? null) };
      const members = t === "distributed" ? [keep, ...(o.members.length > 1 ? o.members.slice(1) : [draft(drives)])] : [keep];
      return { ...o, topology: t, drives, members, volumes: Array.from({ length: drives }, (_, i) => o.volumes[i] ?? bindVolume()) };
    });

  const volumeBody = (v: VolumeDraft): DockerVolume =>
    v.kind === "named" ? { kind: "named", name: v.name.trim(), driver: v.driver.trim() || undefined, options: Object.fromEntries(v.options.filter((o) => o.key.trim()).map((o) => [o.key.trim(), o.value])) } : { kind: "bind" };
  const dockerInstallBody = () => {
    const d = dockerBody(f.docker);
    const volumes = f.volumes.slice(0, f.drives).map(volumeBody);
    return { ...d, network: distributed ? ("host" as const) : d.network, volumes: volumes.some((v) => v.kind === "named") ? volumes : undefined };
  };
  const setVolume = (i: number, p: Partial<VolumeDraft>) => set({ volumes: f.volumes.map((v, j) => (j === i ? { ...v, ...p } : v)) });

  const submit = () => {
    setTried(true);
    if (problemCount || install.isPending) return;
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
        region: f.region.trim() || undefined,
        docker: isDocker ? dockerInstallBody() : undefined,
      },
      {
        onSuccess: (r) => {
          setResult(r ?? {});
          toast(`Installing ${f.name.trim()}`);
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

  const back = (
    <Link to="/storage">Object storage</Link>
  );

  // After submit: the job streams here, with a way on to the store.
  if (result) {
    return (
      <FormPage
        back={back}
        current="Install RustFS"
        title={`Installing ${f.name.trim() || "RustFS"}`}
        description="The install runs as a job. You can leave this page; it keeps running."
        footer={
          <>
            <Button variant="ghost" onClick={() => void navigate({ to: "/storage" })}>
              Back to storage
            </Button>
            {result.store?.id && (
              <Button variant="primary" onClick={() => void navigate({ to: "/storage/$storeId", params: { storeId: result.store?.id ?? "" } })}>
                Open store
              </Button>
            )}
          </>
        }
      >
        <InstallJobs result={result} nodeLabel={(id) => (byId(id) ? nodeName(byId(id) as Server) : id.slice(0, 8))} />
      </FormPage>
    );
  }

  const sections: FormSectionMeta[] = [
    { id: "topology", title: "Topology", icon: Layers, tone: "accent", state: "ok" },
    { id: "basics", title: "Name and release", icon: Tag, tone: "violet", state: state(["name", "slug", "version"], !!f.name.trim(), touched.name) },
    { id: "runtime", title: "Runtime", icon: Cpu, tone: "indigo", state: state(["runtime"], true, touched.nodes) },
    { id: "nodes", title: distributed ? "Nodes and drives" : "Node and data", icon: ServerIcon, tone: "sky", state: state(["nodes"], chosen.length > 0, touched.nodes) },
    { id: "network", title: "Network", icon: Network, tone: "teal", state: state(["s3_port", "console_port", "region"], true, true) },
    ...(isDocker ? [{ id: "docker", title: "Docker", icon: Container, tone: "orange" as const, state: state(["docker", "volumes"], true, true) }] : []),
  ];

  const summary = (
    <SummaryPanel title="What will be created">
      <SummaryRow label="Store">{f.name.trim() || <span className="text-faint">—</span>}</SummaryRow>
      <SummaryRow label="Topology">
        {distributed ? "Distributed (HA)" : "Standalone"}
        {distributed && (
          <span className={`block font-mono text-[11px] tabular-nums ${totalDrives >= MIN_DISTRIBUTED_DRIVES && chosen.length >= 2 ? "text-muted" : "text-bad"}`}>
            {chosen.length} node{chosen.length === 1 ? "" : "s"} × {f.drives} = {totalDrives} drives (min {MIN_DISTRIBUTED_DRIVES})
          </span>
        )}
      </SummaryRow>
      <SummaryRow label={isDocker ? "Container" : "Service"} mono>
        {instance}
      </SummaryRow>
      <SummaryRow label="Runs" mono>
        {isDocker ? image : win ? `rustfs.exe ${f.version || "latest"} (task)` : `rustfs ${f.version || "latest"} (systemd)`}
      </SummaryRow>
      <SummaryRow label="Ports" mono>
        S3 {f.s3_port || "—"}
        {consolePort ? ` · console ${consolePort}` : ""}
      </SummaryRow>
      <SummaryRow label="Region" mono>
        {f.region.trim() || "—"}
      </SummaryRow>
      <SummaryRow label="Environment">
        <StoreOwner store={{ environment_id: f.environment_id || null }} />
      </SummaryRow>
      <div className="space-y-1.5 border-t border-line pt-2">
        {resolved.map((m, i) => (
          <div key={m.key} className="min-w-0 text-xs">
            <div className="flex min-w-0 items-center gap-1.5">
              <HardDrive aria-hidden className="size-3.5 shrink-0 text-sky" />
              <span className="truncate font-medium" title={m.node ? nodeName(m.node) : undefined}>
                {m.node ? nodeName(m.node) : <span className="text-faint">Node {i + 1}: —</span>}
              </span>
            </div>
            <div className="ml-5 space-y-0.5 font-mono text-[11px] break-all text-muted">
              {m.paths.map((p, j) => (
                <div key={j}>
                  {isDocker && f.volumes[j]?.kind === "named" ? `volume ${f.volumes[j]?.name || "—"}` : p}
                </div>
              ))}
              <div className="flex min-w-0 items-center gap-1 text-accent">
                <span className="min-w-0 break-all">{endpointOf(m.host)}</span>
                {m.host && <CopyButton text={endpointOf(m.host)} label="Copy endpoint" />}
              </div>
            </div>
          </div>
        ))}
      </div>
      <div className="border-t border-line pt-2 text-xs text-muted">
        <div className="mb-1 font-medium text-fg">What happens</div>
        <ol className="list-decimal space-y-0.5 pl-4">
          <li>A root credential is generated and stored in the vault.</li>
          <li>
            {isDocker ? `${image} is pulled and started as a container` : win ? "rustfs.exe is downloaded, checksum-verified and registered as a SYSTEM scheduled task" : "The release is downloaded, checksum-verified and installed as a systemd service"}
            {distributed ? ` on each of the ${chosen.length} nodes.` : "."}
          </li>
          <li>The firewall opens the S3{consolePort ? " and console ports" : " port"}. Data paths are created if missing and never deleted on uninstall.</li>
          <li>Forge waits for the health check{distributed ? " on every node" : ""}, then marks the store running.</li>
          {distributed && <li>If drives of one node share a disk, the node's job log warns: the node is the failure unit.</li>}
          <li>If the first install fails everywhere, Forge removes what it set up; the job log stays in Jobs.</li>
        </ol>
      </div>
    </SummaryPanel>
  );

  return (
    <FormPage
      back={back}
      current="Install RustFS"
      title="Install RustFS"
      description="A managed S3-compatible object store on your own nodes. Defaults work for most setups."
      sections={sections}
      summary={summary}
      onSubmit={submit}
      note={
        tried && problemCount > 0 ? (
          <span role="status" className="text-xs text-bad">
            {problemCount} problem{problemCount === 1 ? "" : "s"} to fix above
          </span>
        ) : (
          "Nothing is installed until you press Install."
        )
      }
      footer={
        <>
          <Button type="button" variant="ghost" disabled={install.isPending} onClick={() => void navigate({ to: "/storage" })}>
            Cancel
          </Button>
          <Button type="submit" variant="primary" disabled={install.isPending || (tried && problemCount > 0)}>
            {install.isPending ? <Loader2 className="size-4 animate-spin" /> : <Database className="size-4" />}
            Install
          </Button>
        </>
      }
    >
      <FormSection id="topology" title="Topology" icon={Layers} tone="accent" description="How many nodes hold the data. You can't change this after install.">
        <ChoiceCards
          label="Topology"
          value={f.topology}
          onChange={chooseTopology}
          options={[
            { id: "standalone", title: "Single node", icon: Box, description: "One node, one data path. Simple; no redundancy while the node is down. Linux or Windows." },
            {
              id: "distributed",
              title: "Distributed (HA)",
              icon: Layers,
              description: `Erasure-coded across 2+ Linux nodes. Survives losing a node or drives. Needs ${MIN_DISTRIBUTED_DRIVES}+ drives in total.`,
            },
          ]}
        />
      </FormSection>

      <FormSection id="basics" title="Name and release" icon={Tag} tone="violet" description="What the store is called in Forge and which RustFS version runs.">
        <div className="grid gap-3 sm:grid-cols-3">
          <Field label="Name" hint={show("name", touched.name) ? <FieldError>{show("name", touched.name)}</FieldError> : "Shown in the console."}>
            <Input autoFocus maxLength={64} className={bad(show("name", touched.name))} value={f.name} onChange={(e) => set({ name: e.target.value })} placeholder="media" />
          </Field>
          <Field label="Instance name" hint={always("slug") && (f.slug !== null || tried) ? <FieldError>{always("slug")}</FieldError> : "Service, container and folder name."}>
            <Input className={`font-mono ${bad(f.slug !== null || tried ? always("slug") : undefined)}`} value={slug} onChange={(e) => set({ slug: e.target.value })} />
          </Field>
          <Field label="Release" hint={always("version") ? <FieldError>{always("version")}</FieldError> : "latest, or exact like 1.0.5"}>
            <Input className={`font-mono ${bad(always("version"))}`} value={f.version} onChange={(e) => set({ version: e.target.value })} />
          </Field>
        </div>
        <EnvironmentField value={f.environment_id} onChange={(v) => set({ environment_id: v })} />
      </FormSection>

      <FormSection id="runtime" title="Runtime" icon={Cpu} tone="indigo" description="How RustFS runs on the node.">
        <ChoiceCards
          label="Runtime"
          tone="indigo"
          value={runtime}
          onChange={(v) => set({ runtime: v })}
          options={[
            { id: "native", title: "Native service", icon: Settings2, description: win ? "Windows scheduled task running rustfs.exe." : "A systemd service from the checksum-verified release." },
            {
              id: "docker",
              title: "Docker container",
              icon: Container,
              description: distributed ? "rustfs/rustfs with host networking on every node." : "The rustfs/rustfs image; tune limits and volumes below.",
              why: win ? "Windows nodes run native only." : dockerWhy ? `${dockerWhy}.` : null,
              badge: dockerChecking ? <Loader2 aria-label="Checking Docker" className="size-3.5 animate-spin text-muted" /> : undefined,
            },
          ]}
        />
        <FieldError>{show("runtime", touched.nodes)}</FieldError>
      </FormSection>

      <FormSection
        id="nodes"
        title={distributed ? "Nodes and drives" : "Node and data"}
        icon={ServerIcon}
        tone="sky"
        description={distributed ? "Linux nodes that each hold the same number of drives. Every node must reach every other on its endpoint host and the S3 port." : "Where RustFS runs and where it keeps its data."}
      >
        {distributed ? (
          <div className="space-y-2">
            <label className="flex items-center gap-2 text-xs text-muted">
              Drives per node
              <Input type="number" min={1} max={MAX_DRIVES_PER_NODE} className="!w-16 font-mono" value={f.drives} onChange={(e) => setDrives(Number(e.target.value))} />
              <span className={`font-mono text-[11px] tabular-nums ${totalDrives >= MIN_DISTRIBUTED_DRIVES ? "text-faint" : "text-bad"}`}>
                = {totalDrives} drives · minimum {MIN_DISTRIBUTED_DRIVES}
              </span>
            </label>
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
                  {f.members.map((m, mi) => (
                    <tr key={m.key} className="align-top">
                      <td className="w-48 min-w-[10rem] px-2 py-1.5">
                        <Select aria-label={`Node ${mi + 1}`} value={m.server_id} onChange={(e) => setMember(m.key, { server_id: e.target.value })}>
                          <option value="">{servers.isLoading ? "Loading…" : "Choose a node"}</option>
                          {nodeOptions(m.key)}
                        </Select>
                      </td>
                      <td className="w-44 min-w-[9rem] px-2 py-1.5">
                        <Input aria-label={`Endpoint host for node ${mi + 1}`} className="font-mono !text-xs" value={m.endpoint_host} placeholder="automatic" onChange={(e) => setMember(m.key, { endpoint_host: e.target.value })} />
                      </td>
                      <td className="min-w-[12rem] space-y-1 px-2 py-1.5">
                        {m.paths.map((p, i) => (
                          <Input
                            key={i}
                            aria-label={`Node ${mi + 1} drive ${i + 1} data path`}
                            className="font-mono !text-xs"
                            value={p ?? resolved[mi]?.paths[i] ?? ""}
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
                  ))}
                </tbody>
              </table>
            </div>
            <Button type="button" variant="ghost" disabled={f.members.length >= MAX_NODES} title={f.members.length >= MAX_NODES ? `At most ${MAX_NODES} nodes` : undefined} onClick={() => set({ members: [...f.members, draft(f.drives)] })}>
              <Plus className="size-4" /> Add node
            </Button>
          </div>
        ) : (
          lead && (
            <div className="grid gap-3 sm:grid-cols-2">
              <Field label="Node" hint={win ? "Windows support is implemented but not yet field-tested." : "An online node."}>
                <Select value={lead.server_id} onChange={(e) => setMember(lead.key, { server_id: e.target.value })} disabled={servers.isLoading}>
                  <option value="">{servers.isLoading ? "Loading nodes…" : "Choose a node"}</option>
                  {nodeOptions(lead.key)}
                </Select>
              </Field>
              <Field label="Endpoint host" hint="Optional. Address the control plane uses; defaults to the node's.">
                <Input className="font-mono" value={lead.endpoint_host} placeholder="automatic" onChange={(e) => setMember(lead.key, { endpoint_host: e.target.value })} />
              </Field>
              <div className="sm:col-span-2">
                <Field
                  label="Data path"
                  hint={
                    win
                      ? "Absolute, e.g. D:\\ForgeData\\objstore\\media. Not under \\Windows, \\Program Files, \\ProgramData or \\Users."
                      : "Absolute, e.g. /srv/objstore/media. Not a system folder. Never deleted on uninstall."
                  }
                >
                  <Input className="font-mono" value={lead.paths[0] ?? resolved[0]?.paths[0] ?? ""} onChange={(e) => setMember(lead.key, { paths: [e.target.value] })} />
                </Field>
              </div>
            </div>
          )
        )}
        {servers.error && <ErrorNote error={servers.error} />}
        <FieldError>{show("nodes", touched.nodes)}</FieldError>
        <label className="flex items-start gap-2 text-[13px]">
          <input type="checkbox" className="mt-0.5" checked={f.require_mount} onChange={(e) => set({ require_mount: e.target.checked })} />
          <span>
            Require a dedicated mount
            <span className="block text-xs text-faint">Refuse to install or start unless every data path is its own mount point, so data never lands on the root disk.</span>
          </span>
        </label>
      </FormSection>

      <FormSection id="network" title="Network" icon={Network} tone="teal" description={`The S3 API port${distributed ? " (the same on every node)" : ""}, the region clients see, and the optional web console.`}>
        <div className="grid gap-3 sm:grid-cols-3">
          <Field label="S3 port" hint={always("s3_port") ? <FieldError>{always("s3_port")}</FieldError> : "Default 9000."}>
            <Input type="number" min={1024} max={65535} className={`font-mono ${bad(always("s3_port"))}`} value={f.s3_port} onChange={(e) => set({ s3_port: e.target.value })} />
          </Field>
          <Field label="Region" hint={always("region") ? <FieldError>{always("region")}</FieldError> : "Reported to S3 clients."}>
            <Input className={`font-mono ${bad(always("region"))}`} value={f.region} onChange={(e) => set({ region: e.target.value })} />
          </Field>
          <Field label="Web console" hint={always("console_port") ? <FieldError>{always("console_port")}</FieldError> : f.console ? "RustFS web UI on its own port." : "Off: only the S3 API."}>
            <div className="flex items-center gap-2">
              <label className="flex shrink-0 items-center gap-1.5 text-[13px]">
                <input type="checkbox" checked={f.console} onChange={(e) => set({ console: e.target.checked, console_port: e.target.checked && !f.console_port ? "9001" : f.console_port })} />
                On
              </label>
              <Input aria-label="Console port" type="number" min={1024} max={65535} className={`font-mono ${bad(always("console_port"))}`} disabled={!f.console} value={f.console ? f.console_port : ""} placeholder="—" onChange={(e) => set({ console_port: e.target.value })} />
            </div>
          </Field>
        </div>
      </FormSection>

      {isDocker && (
        <FormSection id="docker" title="Docker" icon={Container} tone="orange" description={`Where each drive's data lives${distributed ? " (the same on every node)" : ""}. Image, limits and environment are under Advanced.`}>
          {f.volumes.slice(0, f.drives).map((v, i) => (
            <div key={i} className="space-y-2">
              {f.drives > 1 && <div className="font-mono text-[11px] text-muted">drive {i + 1}</div>}
              <ChoiceCards
                label={`Drive ${i + 1} volume`}
                tone="orange"
                value={v.kind}
                onChange={(kind) => setVolume(i, { kind })}
                options={[
                  { id: "bind", title: "Host path", icon: FolderTree, description: "Bind-mount the data path above. Easy to back up and inspect." },
                  { id: "named", title: "Named volume", icon: HardDrive, description: "A Docker volume, optionally with a driver (NFS, …). Ignores the data path." },
                ]}
              />
              {v.kind === "named" && (
                <div className="space-y-2 border-l-2 border-orange/40 pl-3">
                  <div className="grid gap-2 sm:grid-cols-2">
                    <Field label="Volume name">
                      <Input className="font-mono" value={v.name} placeholder={`${slug}-d${i + 1}`} onChange={(e) => setVolume(i, { name: e.target.value })} />
                    </Field>
                    <Field label="Driver" hint="Empty = local.">
                      <Input className="font-mono" value={v.driver} placeholder="local" onChange={(e) => setVolume(i, { driver: e.target.value })} />
                    </Field>
                  </div>
                  <KvRows rows={v.options} onChange={(options) => setVolume(i, { options })} label={`drive ${i + 1} driver option`} addLabel="Driver option" />
                </div>
              )}
            </div>
          ))}
          <FieldError>{always("volumes")}</FieldError>
          <details className="rounded-md border border-line" open={!!always("docker") || undefined}>
            <summary className="cursor-pointer px-3 py-2 text-xs font-medium text-muted select-none hover:text-fg">Advanced: image, container name, restart, network, limits, environment</summary>
            <div className="space-y-2 border-t border-line px-3 py-3">
              <DockerFields value={f.docker} onChange={(docker) => set({ docker })} slug={slug} forceHost={distributed} />
            </div>
          </details>
          <FieldError>{always("docker")}</FieldError>
        </FormSection>
      )}
      <ErrorNote error={install.error} />
    </FormPage>
  );
}
