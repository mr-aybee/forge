// Docker options of a managed RustFS store: shared by the install wizard and the settings dialog.
import { Plus, X } from "lucide-react";

import type { DockerRestart, StoreDocker } from "../../api/types";
import { Button, Field, IconButton, Input, Select } from "../ui";

/** The RustFS variables Forge lets you set; credential, address, volume and console variables never. */
export const ENV_KEYS = [
  "RUSTFS_SERVER_DOMAINS",
  "RUSTFS_CORS_ALLOWED_ORIGINS",
  "RUSTFS_CONSOLE_CORS_ALLOWED_ORIGINS",
  "RUSTFS_OBS_LOGGER_LEVEL",
  "RUSTFS_OBS_ENVIRONMENT",
  "RUSTFS_COMPRESSION_ENABLED",
  "RUSTFS_STORAGE_CLASS_STANDARD",
  "RUSTFS_STORAGE_CLASS_RRS",
  "RUSTFS_SCANNER_SPEED",
] as const;

export const VOLUME_NAME = /^[a-zA-Z0-9][a-zA-Z0-9_.-]{0,63}$/;
const IMAGE = /^[a-z0-9]([a-z0-9._/:-]*[a-z0-9])?$/;
const CONTAINER = /^[a-zA-Z0-9][a-zA-Z0-9_.-]{0,127}$/;
const OPT_KEY = /^[A-Za-z0-9_.-]{1,64}$/;
export const REGION = /^[a-z0-9-]{1,32}$/;

export const RESTARTS: { id: DockerRestart; label: string }[] = [
  { id: "unless-stopped", label: "unless-stopped" },
  { id: "always", label: "always" },
  { id: "on-failure", label: "on-failure" },
  { id: "no", label: "no" },
];

export interface Kv {
  /** Stable React key. */
  id: number;
  key: string;
  value: string;
}

export interface DockerDraft {
  image: string;
  container_name: string;
  restart: DockerRestart;
  network: "bridge" | "host";
  memory_mb: string;
  /** In CPUs, e.g. 1.5; sent as `cpu_millis`. */
  cpus: string;
  env: Kv[];
}

let kvSeq = 0;
export const kvRow = (key = "", value = ""): Kv => ({ id: ++kvSeq, key, value });
export const kvList = (m?: Record<string, string> | null): Kv[] => Object.entries(m ?? {}).map(([key, value]) => kvRow(key, value));
export const kvMap = (l: Kv[]): Record<string, string> => Object.fromEntries(l.filter((r) => r.key.trim()).map((r) => [r.key.trim(), r.value]));

export const dockerDraft = (d?: StoreDocker | null): DockerDraft => ({
  image: d?.image || "rustfs/rustfs",
  container_name: d?.container_name ?? "",
  restart: d?.restart ?? "unless-stopped",
  network: d?.network === "host" ? "host" : "bridge",
  memory_mb: typeof d?.memory_mb === "number" ? String(d.memory_mb) : "",
  cpus: typeof d?.cpu_millis === "number" ? String(d.cpu_millis / 1000) : "",
  env: kvList(d?.env),
});

const memory = (v: string) => (v.trim() ? Number(v) : null);
const cpuMillis = (v: string) => (v.trim() ? Math.round(Number(v) * 1000) : null);

/** Reasons the draft cannot be sent; empty when it is fine. */
export function dockerProblems(d: DockerDraft, opts: { containerName?: boolean } = {}): string[] {
  const p: string[] = [];
  if (!IMAGE.test(d.image.trim())) p.push("Image: a repository such as rustfs/rustfs, without a tag (the tag is the release).");
  if ((d.image.trim().split("/").pop() ?? "").includes(":") || d.image.includes("@")) p.push("Image: leave the tag out; set the release instead.");
  if (opts.containerName && d.container_name.trim() && !CONTAINER.test(d.container_name.trim())) p.push("Container name: letters, digits, _ . - (letter or digit first).");
  const mem = memory(d.memory_mb);
  if (mem !== null && (!Number.isInteger(mem) || mem < 256 || mem > 1_048_576)) p.push("Memory: a whole number of MB, at least 256.");
  const cpu = cpuMillis(d.cpus);
  if (cpu !== null && (!Number.isFinite(cpu) || cpu < 100 || cpu > 256_000)) p.push("CPUs: at least 0.1, e.g. 1.5.");
  const keys = d.env.map((r) => r.key.trim()).filter(Boolean);
  if (keys.some((k) => !(ENV_KEYS as readonly string[]).includes(k))) p.push("Environment: only the listed RUSTFS_ variables are allowed.");
  if (new Set(keys).size !== keys.length) p.push("Environment: each variable once.");
  if (d.env.some((r) => !r.key.trim() && r.value.trim())) p.push("Environment: choose a variable for every value.");
  return p;
}

/** The request body; `undefined` fields are left to the server's defaults. */
export function dockerBody(d: DockerDraft): StoreDocker {
  return {
    image: d.image.trim() || undefined,
    container_name: d.container_name.trim() || undefined,
    restart: d.restart,
    network: d.network,
    memory_mb: memory(d.memory_mb),
    cpu_millis: cpuMillis(d.cpus),
    env: kvMap(d.env),
  };
}

export const optProblems = (rows: Kv[], what: string) => {
  const keys = rows.map((r) => r.key.trim()).filter(Boolean);
  const p: string[] = [];
  if (keys.some((k) => !OPT_KEY.test(k))) p.push(`${what}: keys are letters, digits, _ . - (max 64).`);
  if (new Set(keys).size !== keys.length) p.push(`${what}: each key once.`);
  return p;
};

/** Key/value rows. With `keys`, the key is chosen from that list. */
export function KvRows({ rows, onChange, keys, label, addLabel, disabled }: { rows: Kv[]; onChange: (r: Kv[]) => void; keys?: readonly string[]; label: string; addLabel: string; disabled?: boolean }) {
  const set = (i: number, p: Partial<Kv>) => onChange(rows.map((r, j) => (j === i ? { ...r, ...p } : r)));
  const used = new Set(rows.map((r) => r.key));
  return (
    <div className="space-y-1">
      {rows.map((r, i) => (
        <div key={r.id} className="flex min-w-0 items-center gap-1.5">
          {keys ? (
            <Select aria-label={`${label} ${i + 1} name`} className="min-w-0 flex-1 font-mono !text-xs" value={r.key} disabled={disabled} onChange={(e) => set(i, { key: e.target.value })}>
              <option value="">Choose a variable</option>
              {keys.map((k) => (
                <option key={k} value={k} disabled={k !== r.key && used.has(k)}>
                  {k}
                </option>
              ))}
            </Select>
          ) : (
            <Input aria-label={`${label} ${i + 1} key`} className="min-w-0 flex-1 font-mono !text-xs" value={r.key} placeholder="key" disabled={disabled} onChange={(e) => set(i, { key: e.target.value })} />
          )}
          <Input aria-label={`${label} ${i + 1} value`} className="min-w-0 flex-1 font-mono !text-xs" value={r.value} placeholder="value" disabled={disabled} onChange={(e) => set(i, { value: e.target.value })} />
          <IconButton type="button" label={`Remove ${label} ${i + 1}`} disabled={disabled} onClick={() => onChange(rows.filter((_, j) => j !== i))}>
            <X className="size-3.5" />
          </IconButton>
        </div>
      ))}
      <Button type="button" variant="ghost" disabled={disabled || (!!keys && rows.length >= keys.length)} onClick={() => onChange([...rows, kvRow()])}>
        <Plus className="size-4" /> {addLabel}
      </Button>
    </div>
  );
}

/**
 * Image, container, restart, network, limits and environment.
 * `fixed`: container name and network cannot change (settings). `forceHost`: distributed stores use host networking.
 */
export function DockerFields({ value, onChange, slug, fixed, forceHost }: { value: DockerDraft; onChange: (d: DockerDraft) => void; slug?: string; fixed?: boolean; forceHost?: boolean }) {
  const set = (p: Partial<DockerDraft>) => onChange({ ...value, ...p });
  const network = forceHost ? "host" : value.network;
  return (
    <div className="space-y-3">
      <div className="grid gap-3 sm:grid-cols-3">
        <Field label="Image" hint="Repository; the tag is the release.">
          <Input className="font-mono" value={value.image} placeholder="rustfs/rustfs" onChange={(e) => set({ image: e.target.value })} />
        </Field>
        <Field label="Container name" hint={fixed ? "Fixed after install." : undefined}>
          <Input className="font-mono" value={value.container_name} disabled={fixed} placeholder={`forge-objstore-${slug || "<slug>"}`} onChange={(e) => set({ container_name: e.target.value })} />
        </Field>
        <Field label="Restart policy">
          <Select value={value.restart} onChange={(e) => set({ restart: (RESTARTS.find((r) => r.id === e.target.value)?.id ?? "unless-stopped") as DockerRestart })}>
            {RESTARTS.map((r) => (
              <option key={r.id} value={r.id}>
                {r.label}
              </option>
            ))}
          </Select>
        </Field>
        <Field label="Network" hint={forceHost ? "Distributed stores always use host networking." : fixed ? "Fixed after install." : network === "host" ? "Ports bind on the node directly." : "Ports are published."}>
          <Select value={network} disabled={forceHost || fixed} title={forceHost ? "Distributed stores always use host networking" : undefined} onChange={(e) => set({ network: e.target.value === "host" ? "host" : "bridge" })}>
            <option value="bridge">bridge</option>
            <option value="host">host</option>
          </Select>
        </Field>
        <Field label="Memory (MB)" hint="Empty = no limit.">
          <Input type="number" min={256} step={1} className="font-mono" value={value.memory_mb} placeholder="—" onChange={(e) => set({ memory_mb: e.target.value })} />
        </Field>
        <Field label="CPUs" hint="e.g. 1.5. Empty = no limit.">
          <Input type="number" min={0.1} step={0.1} className="font-mono" value={value.cpus} placeholder="—" onChange={(e) => set({ cpus: e.target.value })} />
        </Field>
      </div>
      <div>
        <div className="mb-1 text-xs font-medium text-muted">Environment</div>
        <KvRows rows={value.env} onChange={(env) => set({ env })} keys={ENV_KEYS} label="variable" addLabel="Add variable" />
        <p className="mt-1 text-[11px] text-faint">Credential, address, volume and console variables are set by Forge and cannot be overridden.</p>
      </div>
    </div>
  );
}
