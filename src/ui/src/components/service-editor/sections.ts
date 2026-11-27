/**
 * Metadata and pure logic of the service editor: the two kinds, the sections
 * each kind shows (in rail order), per-section status for the rail markers,
 * and the conversions between a service and its editable input. No React.
 */
import { Activity, Boxes, KeyRound, Cpu, HardDrive, Network, Package, ServerCog, Settings2, SquareTerminal, Variable, Container, IdCard, Layers, Shield, Play, type LucideIcon } from "lucide-react";

import { defaultSwarmSettings, normSwarmSettings } from "../../api/swarm";
import type { EditableRuntime, RestartPolicy, SecretFileRef, Service, ServiceInput, ServiceRuntime } from "../../api/types";

/** The kinds a user picks between, shown as two large cards. */
export const KINDS: { id: EditableRuntime; label: string; icon: LucideIcon; description: string; needs: string }[] = [
  {
    id: "container",
    label: "Container",
    icon: Boxes,
    description: "A Docker image with published ports, volumes and environment.",
    needs: "Needs an image. Forge installs Docker Engine on the node when it is missing.",
  },
  {
    id: "systemd",
    label: "Systemd",
    icon: ServerCog,
    description: "A long-running process under systemd, run as its own user and restarted when it exits.",
    needs: "Needs a start command. Runtimes such as Node.js or .NET are installed from the App Store.",
  },
];

/** Label and icon of every runtime a service can have, including read-only compose services. */
export const RUNTIMES: { id: ServiceRuntime; label: string; icon: LucideIcon; help: string }[] = [
  ...KINDS.map((k) => ({ id: k.id as ServiceRuntime, label: k.label, icon: k.icon, help: k.description })),
  { id: "compose-service", label: "Compose service", icon: Container, help: "Registered by a compose stack; managed on its stack." },
];

export type SectionId = "basics" | "runtime" | "start" | "resources" | "env" | "storage" | "health" | "advanced" | "image" | "network" | "run" | "volumes" | "security" | "swarm" | "files";

export interface SectionMeta {
  id: SectionId;
  label: string;
  icon: LucideIcon;
  /** One line under the card title. */
  hint: string;
}

const S: Record<SectionId, SectionMeta> = {
  basics: { id: "basics", label: "Basics", icon: IdCard, hint: "Name, where it lives and the nodes it runs on." },
  runtime: { id: "runtime", label: "Runtime", icon: Package, hint: "Language runtimes installed on each node before the service is deployed." },
  start: { id: "start", label: "Start command", icon: SquareTerminal, hint: "How systemd starts the process, the port it listens on and its web console link." },
  image: { id: "image", label: "Image", icon: Boxes, hint: "The container image to run, and overrides of its CMD and ENTRYPOINT." },
  run: { id: "run", label: "Runtime", icon: Play, hint: "Restart policy, user, host name, init and stop behaviour." },
  network: { id: "network", label: "Network", icon: Network, hint: "Published ports, cluster network, extra networks, hosts, DNS and the web console link." },
  security: { id: "security", label: "Security", icon: Shield, hint: "Root filesystem, Linux capabilities, devices and kernel parameters." },
  resources: { id: "resources", label: "Limits", icon: Cpu, hint: "Limits per node. Empty means no limit." },
  env: { id: "env", label: "Environment variables", icon: Variable, hint: "Written to the service's environment file (0600). Secrets are sealed and never shown again." },
  storage: { id: "storage", label: "Storage", icon: HardDrive, hint: "Where the service keeps code and data on the node." },
  volumes: { id: "volumes", label: "Storage", icon: HardDrive, hint: "Bind mounts and in-memory tmpfs mounts." },
  health: { id: "health", label: "Health checks", icon: Activity, hint: "How Forge and Docker decide that the service is healthy." },
  advanced: { id: "advanced", label: "Advanced", icon: Settings2, hint: "Rarely needed options." },
  files: { id: "files", label: "Secrets & configs", icon: KeyRound, hint: "Credential vault items mounted as read-only files. Only names are stored; values are written on the node at deploy." },
  swarm: { id: "swarm", label: "Swarm", icon: Layers, hint: "Replicas, where tasks may run, and how updates and restarts roll out." },
};

export const SECTIONS: Record<EditableRuntime, SectionMeta[]> = {
  systemd: [S.basics, S.runtime, S.start, S.resources, S.env, S.files, S.storage, S.health, S.advanced],
  container: [S.basics, S.image, S.run, S.network, S.volumes, S.resources, S.security, S.env, S.files, S.health, { ...S.advanced, hint: "Labels, log rotation, and the whole container as compose YAML." }],
};

/** The sections of an input: a container in a Swarm cluster (`swarm` set) also gets the Swarm section. */
export const sectionsFor = (v: ServiceInput): SectionMeta[] =>
  v.runtime === "container" && v.swarm ? [...SECTIONS.container.slice(0, 3), S.swarm, ...SECTIONS.container.slice(3).filter((s) => s.id !== "files")] : SECTIONS[v.runtime];

/** The input for an environment in (or out of) a Swarm cluster: container with settings, or no `swarm` at all. */
export function forSwarm(v: ServiceInput, swarm: boolean): ServiceInput {
  if (!swarm) {
    if (v.swarm === undefined) return v;
    const { swarm: _drop, ...rest } = v;
    return rest;
  }
  const c = v.runtime === "container" ? v : switchKind(v, "container");
  return c.swarm ? c : { ...c, swarm: defaultSwarmSettings() };
}

/** Docker placement constraint: key, `==` or `!=`, value. */
/** A Docker service name Forge accepts (as `docker_name_ok` on the server). */
export const DOCKER_NAME_RE = /^(?!-)[a-z0-9-]{1,61}(?<!-)$/;
export const CONSTRAINT_RE = /^(node\.(id|hostname|role|platform\.os|platform\.arch)|(node|engine)\.labels\.[A-Za-z0-9_.-]+)\s*(==|!=)\s*\S+$/;
export const SPREAD_RE = /^node\.labels\.[A-Za-z0-9_.-]+$/;

export const sectionDomId = (id: SectionId) => `service-section-${id}`;

// ------------------------------------------------------------------ validation

export const KEY_RE = /^[A-Za-z_][A-Za-z0-9_]*$/;
export const PORT_RE = /^((\d{1,3}\.){3}\d{1,3}:)?\d{1,5}:\d{1,5}(\/(tcp|udp))?$/;
export const VOL_RE = /^\/[^:\s]*:\/[^:\s]*(:(ro|rw))?$/;
export const HOST_RE = /^[A-Za-z0-9.-]+:(\d{1,3}\.){3}\d{1,3}$/;
/** Server rule for service names. */
export const NAME_RE = /^[a-z][a-z0-9-]{0,39}$/;
/** Mirrors forge_catalog::container_opts (the server is authoritative). */
export const CAP_RE = /^(CAP_)?[A-Z_]{1,32}$/;
export const SYSCTL_RE = /^(?=.{1,128}$)[A-Za-z0-9_-]+(\.[A-Za-z0-9_-]+)+$/;
export const NETWORK_RE = /^[A-Za-z0-9][A-Za-z0-9_.-]{0,63}$/;
export const PERMS_RE = /^[rwm]{0,3}$/;
export const ULIMITS = ["core", "cpu", "data", "fsize", "locks", "memlock", "msgqueue", "nice", "nofile", "nproc", "rss", "rtprio", "rttime", "sigpending", "stack"] as const;
export const RESTART_POLICIES: { id: RestartPolicy; label: string }[] = [
  { id: "unless-stopped", label: "Unless stopped" },
  { id: "always", label: "Always" },
  { id: "on-failure", label: "On failure" },
  { id: "no", label: "Never" },
];
/** An absolute container path without `..`, `:`, `,` or spaces. */
export const pathOk = (p: string) => p.startsWith("/") && p.length <= 512 && !p.split("/").includes("..") && !/[: ,\n\r\0]/.test(p);
export const ipOk = (s: string) => {
  const v4 = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/.exec(s);
  if (v4) return v4.slice(1).every((o) => Number(o) <= 255);
  return s.includes(":") && /^[0-9A-Fa-f:.]{2,45}$/.test(s);
};
export const ulimitOk = (u: { soft: number; hard: number }) => Number.isInteger(u.soft) && Number.isInteger(u.hard) && u.soft >= -1 && u.hard >= -1 && (u.hard === -1 || (u.soft !== -1 && u.soft <= u.hard));
/** The effective restart policy of an input (legacy boolean when unset). */
export const policyOf = (v: Pick<ServiceInput, "restart" | "restart_policy">): RestartPolicy => v.restart_policy ?? (v.restart ? "unless-stopped" : "no");
/** Settings that need stack.privileged (admin with MFA) to deploy. */
export const privilegedOf = (v: ServiceInput): string[] => [
  ...v.cap_add.filter(Boolean).map((c) => `cap_add:${c.replace(/^CAP_/, "")}`),
  ...v.devices.filter((d) => d.host).map((d) => `device:${d.host}`),
  ...v.sysctls.filter(([k]) => k).map(([k]) => `sysctl:${k}`),
];

/** Mirrors forge_catalog::console_link::ConsoleLink::validate (the server is authoritative). */
export const consolePathOk = (p: string) => p.startsWith("/") && p.length <= 512 && !/[\s\\]/.test(p);
/** Problems of the console link, shown on the section that holds it. */
const consoleIssues = (v: ServiceInput): string[] => {
  const c = v.console;
  if (!c) return [];
  return [
    ...(!Number.isInteger(c.port) || c.port < 1 || c.port > 65535 ? ["Web console port: 1 to 65535."] : []),
    ...(!consolePathOk(c.path ?? "/") ? ["Console path: starts with /, no spaces."] : []),
  ];
};

/** The file name of a secret file: its target's last part (as on the server). */
export const fileName = (f: SecretFileRef) => f.target.split("/").pop() ?? "";
/** Mirrors forge_catalog::secret_files::check_target (the server is authoritative). */
export const targetOk = (t: string) => {
  if (!t.startsWith("/") || t.length > 255) return false;
  const parts = t.slice(1).split("/");
  return parts.every((p) => p && p !== "." && p !== ".." && /^[A-Za-z0-9._@+-]+$/.test(p)) && !["proc", "sys", "dev"].includes(parts[0] ?? "");
};
/** The default target of a new file for a vault item name. */
export const defaultTarget = (kind: SecretFileRef["kind"], item: string) => {
  const name = item.trim().toLowerCase().replace(/[^a-z0-9._-]+/g, "_") || "file";
  return kind === "secret" ? `/run/secrets/${name}` : `/etc/${name}`;
};

export type SectionStatus = "complete" | "needs" | "error";

/** What the Basics section holds outside the service input (create only). */
export interface BasicsState {
  /** Create: the chosen environment and name. Edit: always valid. */
  mode: "create" | "edit";
  environment: string;
  name: string;
}

/** Problems of one section: `needs` for missing required input, `error` for invalid input. */
export function sectionIssues(id: SectionId, v: ServiceInput, b: BasicsState): { status: SectionStatus; messages: string[] } {
  const errors: string[] = [];
  const needs: string[] = [];
  const dupes = v.env.map((e) => e.key).filter((k, i, a) => k && a.indexOf(k) !== i);
  switch (id) {
    case "basics":
      if (b.mode === "create") {
        if (!b.environment) needs.push("Choose a project and an environment.");
        if (!b.name) needs.push("Enter a name.");
        else if (!NAME_RE.test(b.name)) errors.push("Name: lower-case letters, digits and dashes; start with a letter.");
      }
      break;
    case "start":
      if (!v.command.trim()) needs.push("Enter the start command.");
      if (v.listen_port != null && (v.listen_port < 1 || v.listen_port > 65535)) errors.push("Listening port: 1 to 65535.");
      errors.push(...consoleIssues(v));
      break;
    case "image":
      if (!v.image.trim()) needs.push("Enter an image.");
      else if (/\s/.test(v.image)) errors.push("Image: no spaces.");
      break;
    case "network":
      if (v.ports.some((p) => p && !PORT_RE.test(p))) errors.push("Ports: use host:container or ip:host:container[/udp].");
      if (v.extra_hosts.some((h) => h && !HOST_RE.test(h))) errors.push("Extra hosts: use name:ipv4.");
      if (v.dns.some((d) => d && !ipOk(d))) errors.push("DNS servers: IP addresses only.");
      if (v.dns.filter(Boolean).length > 8) errors.push("DNS servers: at most 8.");
      if (v.networks.some((n) => n && (!NETWORK_RE.test(n) || n === "host"))) errors.push("Networks: existing Docker network names (letters, digits, _ . -; not host).");
      if (v.networks.filter(Boolean).length > 10) errors.push("Networks: at most 10.");
      errors.push(...consoleIssues(v));
      break;
    case "run":
      if (v.stop_timeout_secs < 0 || v.stop_timeout_secs > 3600) errors.push("Stop timeout: 0 to 3600 seconds.");
      break;
    case "resources": {
      if (v.cpu_millis < 0 || v.memory_mb < 0) errors.push("Limits cannot be negative.");
      const names = v.ulimits.map((u) => u.name);
      if (names.some((n, i) => names.indexOf(n) !== i)) errors.push("Ulimits: each name once.");
      if (v.ulimits.some((u) => !ulimitOk(u))) errors.push("Ulimits: soft must not exceed hard (-1 = unlimited).");
      break;
    }
    case "security":
      if ([...v.cap_add, ...v.cap_drop].some((c) => c && !CAP_RE.test(c))) errors.push("Capabilities: upper-case names such as NET_ADMIN, or ALL.");
      if (v.cap_add.length + v.cap_drop.length > 64) errors.push("Capabilities: at most 64.");
      if (v.devices.some((d) => (d.host || d.container) && (!d.host.startsWith("/dev/") || !pathOk(d.host) || (!!d.container && !pathOk(d.container)) || !PERMS_RE.test(d.permissions ?? ""))))
        errors.push("Devices: a /dev/ host path, an absolute container path, permissions from rwm.");
      if (v.devices.length > 20) errors.push("Devices: at most 20.");
      if (v.sysctls.some(([k, val]) => (k || val) && (!SYSCTL_RE.test(k) || !val || val.length > 256))) errors.push("Sysctls: dotted keys such as net.core.somaxconn, each with a value.");
      if (v.sysctls.length > 50) errors.push("Sysctls: at most 50.");
      break;
    case "env":
      if (v.env.some((e) => e.key && !KEY_RE.test(e.key))) errors.push("Names: letters, digits and underscores; not starting with a digit.");
      if (v.env.some((e) => !e.key && (e.value ?? "") !== "")) needs.push("A variable has a value but no name.");
      if (dupes.length) errors.push(`Duplicate names: ${[...new Set(dupes)].join(", ")}.`);
      break;
    case "volumes":
      if (v.volumes.some((p) => p && !VOL_RE.test(p))) errors.push("Volumes: use /host/path:/container/path[:ro].");
      if (v.tmpfs.some((t) => t.target && (!pathOk(t.target) || t.size_mb < 0 || t.size_mb > 1048576))) errors.push("tmpfs: an absolute container path and a size up to 1 TiB.");
      if (v.tmpfs.length > 20) errors.push("tmpfs: at most 20 mounts.");
      break;
    case "health":
      if (v.runtime === "container" && v.healthcheck && !v.healthcheck.command.trim()) needs.push("Enter the health check command, or turn it off.");
      break;
    case "advanced":
      if (v.log_max_mb < 0) errors.push("Log rotation cannot be negative.");
      break;
    case "swarm": {
      const w = v.swarm;
      if (!w) break;
      if (w.name && !DOCKER_NAME_RE.test(w.name)) errors.push("Docker service name: lowercase letters, digits and -, up to 61 characters.");
      if (w.mode === "replicated" && (!Number.isInteger(w.replicas) || w.replicas < 1 || w.replicas > 1000)) errors.push("Replicas: 1 to 1000.");
      if (w.constraints.some((c) => c && !CONSTRAINT_RE.test(c))) errors.push("Constraints: enter a value for each row.");
      if (w.spread.filter(Boolean).length > 4) errors.push("Spread: at most 4 labels.");
      if (w.spread.some((x) => x && !SPREAD_RE.test(x))) errors.push("Spread: use node.labels.<name>.");
      if (w.max_per_node < 0 || w.update.parallelism < 0 || w.restart.max_attempts < 0) errors.push("Counts cannot be negative.");
      if (w.update.max_failure_ratio < 0 || w.update.max_failure_ratio > 1) errors.push("Failure ratio: 0 to 1.");
      break;
    }
    case "files": {
      const names = v.secret_files.map(fileName);
      if (v.secret_files.some((f) => !f.item.trim())) needs.push("Pick a vault item for every file.");
      if (v.secret_files.some((f) => !targetOk(f.target))) errors.push("Targets: absolute paths of plain names, not under /proc, /sys or /dev.");
      if (names.some((n, i) => n && names.indexOf(n) !== i)) errors.push("Two files have the same file name.");
      if (v.secret_files.some((f) => f.mode !== undefined && (!Number.isInteger(f.mode) || f.mode < 0 || f.mode > 0o777))) errors.push("Mode: octal permission bits up to 0777.");
      if (v.secret_files.length > 50) errors.push("At most 50 files.");
      break;
    }
    case "runtime":
    case "storage":
      break;
  }
  if ((id === "run" || id === "start") && v.restart_policy === "on-failure" && (!Number.isInteger(v.restart_max_attempts) || v.restart_max_attempts < 0 || v.restart_max_attempts > 1000))
    errors.push("Restart attempts: 0 (unlimited) to 1000.");
  return { status: errors.length ? "error" : needs.length ? "needs" : "complete", messages: [...errors, ...needs] };
}

/** Every problem of the input for its kind, for the action bar. */
export function allIssues(v: ServiceInput, b: BasicsState): string[] {
  return sectionsFor(v).flatMap((s) => sectionIssues(s.id, v, b).messages);
}

// ------------------------------------------------------------------ conversions

export const emptyInput = (runtime: EditableRuntime = "container"): ServiceInput => ({
  runtime,
  command: "",
  working_dir: "",
  listen_port: null,
  image: "",
  ports: [],
  volumes: [],
  env_files: [],
  restart: true,
  env: [],
  dependencies: [],
  cpu_millis: 0,
  memory_mb: 0,
  entrypoint: "",
  user: "",
  hostname: "",
  labels: [],
  extra_hosts: [],
  healthcheck: null,
  stop_timeout_secs: 0,
  read_only: false,
  log_max_mb: 0,
  restart_max_attempts: 0,
  cap_add: [],
  cap_drop: [],
  ulimits: [],
  tmpfs: [],
  dns: [],
  sysctls: [],
  devices: [],
  init: false,
  networks: [],
  secret_files: [],
});

const arr = <T,>(v: T[] | undefined | null): T[] => (Array.isArray(v) ? v : []);
const isStr = (x: unknown): x is string => typeof x === "string";
const num = (x: unknown) => (typeof x === "number" && Number.isFinite(x) ? x : 0);
const isPolicy = (x: unknown): x is RestartPolicy => x === "no" || x === "always" || x === "unless-stopped" || x === "on-failure";

/** The editable input of a service; every field defaulted so a partial spec cannot break the editor. */
export function toInput(a: Service): ServiceInput {
  const s = a.spec ?? ({} as Service["spec"]);
  return {
    runtime: a.runtime === "container" ? "container" : "systemd",
    command: s.command ?? "",
    working_dir: s.working_dir ?? "",
    listen_port: s.listen_port ?? null,
    image: s.image ?? "",
    ports: arr(s.ports),
    volumes: arr(s.volumes),
    env_files: arr(s.env_files),
    restart: s.restart !== false,
    env: arr(a.env).map((e) => ({ key: e.key, value: e.secret ? null : (e.value ?? ""), secret: e.secret })),
    dependencies: arr(s.dependencies),
    cpu_millis: s.cpu_millis ?? 0,
    memory_mb: s.memory_mb ?? 0,
    entrypoint: s.entrypoint ?? "",
    user: s.user ?? "",
    hostname: s.hostname ?? "",
    labels: arr(s.labels),
    extra_hosts: arr(s.extra_hosts),
    healthcheck: s.healthcheck ?? null,
    stop_timeout_secs: s.stop_timeout_secs ?? 0,
    read_only: s.read_only ?? false,
    log_max_mb: s.log_max_mb ?? 0,
    ...(isPolicy(s.restart_policy) ? { restart_policy: s.restart_policy } : {}),
    restart_max_attempts: num(s.restart_max_attempts),
    cap_add: arr(s.cap_add).filter(isStr),
    cap_drop: arr(s.cap_drop).filter(isStr),
    ulimits: arr(s.ulimits).filter((u) => u && typeof u.name === "string").map((u) => ({ name: u.name, soft: num(u.soft), hard: num(u.hard) })),
    tmpfs: arr(s.tmpfs).filter((t) => t && typeof t.target === "string").map((t) => ({ target: t.target, size_mb: num(t.size_mb) })),
    dns: arr(s.dns).filter(isStr),
    sysctls: arr(s.sysctls).filter((p): p is [string, string] => Array.isArray(p)).map(([k, v]) => [String(k ?? ""), String(v ?? "")] as [string, string]),
    devices: arr(s.devices).filter((d) => d && typeof d.host === "string").map((d) => ({ host: d.host, container: d.container ?? "", permissions: d.permissions ?? "" })),
    init: s.init === true,
    networks: arr(s.networks).filter(isStr),
    secret_files: arr(s.secret_files).filter((f) => f && typeof f.item === "string" && typeof f.target === "string"),
    ...(s.console && typeof s.console.port === "number" ? { console: { port: s.console.port, path: s.console.path ?? "/", scheme: s.console.scheme === "https" ? "https" : "http" } } : {}),
    ...(typeof a.version === "number" ? { version: a.version } : {}),
    ...(a.runtime === "container" && s.mesh === false ? { mesh: false as const } : {}),
    ...(s.swarm ? { swarm: normSwarmSettings(s.swarm) } : {}),
  };
}

/** What changing the kind clears, for the confirmation. */
export const CLEARED_ON_SWITCH: Record<EditableRuntime, string> = {
  systemd: "the image, ports, volumes, entrypoint, user, host name, labels, extra hosts, Docker health check, stop timeout, read-only flag, log size, and the network, storage and security options",
  container: "the runtimes, working directory and listening port",
};

/** The input with the other kind's fields cleared: the server rejects container-only fields on systemd services. */
export function switchKind(v: ServiceInput, kind: EditableRuntime): ServiceInput {
  const empty = emptyInput(kind);
  if (kind === "systemd") {
    const { mesh: _mesh, ...rest } = v;
    return { ...rest, runtime: kind, image: "", ports: [], volumes: [], entrypoint: "", user: "", hostname: "", labels: [], extra_hosts: [], healthcheck: null, stop_timeout_secs: 0, read_only: false, log_max_mb: 0, cap_add: [], cap_drop: [], ulimits: [], tmpfs: [], dns: [], sysctls: [], devices: [], init: false, networks: [] };
  }
  return { ...v, runtime: kind, dependencies: empty.dependencies, working_dir: "", listen_port: null };
}

/** Drops empty rows of the list editors before saving. */
export function cleanInput(v: ServiceInput): ServiceInput {
  return {
    ...v,
    ports: v.ports.filter(Boolean),
    volumes: v.volumes.filter(Boolean),
    extra_hosts: v.extra_hosts.filter(Boolean),
    cap_add: v.cap_add.filter(Boolean),
    cap_drop: v.cap_drop.filter(Boolean),
    dns: v.dns.filter(Boolean),
    networks: v.networks.filter(Boolean),
    tmpfs: v.tmpfs.filter((t) => t.target),
    sysctls: v.sysctls.filter(([k]) => k),
    devices: v.devices.filter((d) => d.host).map((d) => ({ host: d.host, ...(d.container ? { container: d.container } : {}), ...(d.permissions ? { permissions: d.permissions } : {}) })),
    // Older servers read only the boolean; keep it in step with the policy.
    ...(v.restart_policy ? { restart: v.restart_policy !== "no", restart_max_attempts: v.restart_policy === "on-failure" ? v.restart_max_attempts : 0 } : { restart_max_attempts: 0 }),
    env: v.env.filter((e) => e.key),
    ...(v.swarm ? { swarm: { ...v.swarm, constraints: v.swarm.constraints.filter(Boolean), spread: v.swarm.spread.filter(Boolean) } } : {}),
  };
}

// ------------------------------------------------------------------ .env

/** Parses `.env` text: `KEY=value`, `export KEY=value`, quotes, comments. */
export function parseDotenv(text: string): { key: string; value: string }[] {
  const out: { key: string; value: string }[] = [];
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    if (!line || line.startsWith("#")) continue;
    const m = /^(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*)$/.exec(line);
    if (!m) continue;
    let v = m[2] ?? "";
    if ((v.startsWith('"') && v.endsWith('"')) || (v.startsWith("'") && v.endsWith("'"))) v = v.slice(1, -1);
    out.push({ key: m[1] ?? "", value: v });
  }
  return out;
}

export const looksSecret = (k: string) => /PASS|SECRET|TOKEN|KEY|PRIVATE|CREDENTIAL/i.test(k);
/** Stands for a stored secret in text and YAML views. */
export const SEALED = "<sealed>";

export const plural = (n: number, w: string) => `${n} ${w}${n === 1 ? "" : "s"}`;

// ------------------------------------------------------------------ on the node

/** The service's own Linux account (D-081): `<environment>-<service>`, or a shortened name with a hash when that exceeds 32 characters. */
export function serviceUser(environment: string, service: string): string {
  const name = `${environment}-${service}`;
  return name.length <= 32 ? name : `${name.slice(0, 25).replace(/-+$/, "")}-<hash>`;
}

/** Paths and names of a service on its nodes. */
export function serviceFacts(environment: string, service: string): { home: string; envFile: string; unit: string; user: string; container: string } {
  const env = environment || "<environment>";
  const svc = service || "<service>";
  return {
    home: `/srv/forge/apps/${env}/${svc}`,
    envFile: `/etc/forge/apps/${env}/${svc}.env`,
    unit: `forge-app-${env}-${svc}.service`,
    user: serviceUser(env, svc),
    container: `forge-app-${env}-${svc}`,
  };
}
