/**
 * Metadata and pure logic of the service editor: the two kinds, the sections
 * each kind shows (in rail order), per-section status for the rail markers,
 * and the conversions between a service and its editable input. No React.
 */
import { Activity, Boxes, Cpu, HardDrive, Network, Package, ServerCog, Settings2, SquareTerminal, Variable, Container, IdCard, Layers, type LucideIcon } from "lucide-react";

import { defaultSwarmSettings, normSwarmSettings } from "../../api/swarm";
import type { EditableRuntime, Service, ServiceInput, ServiceRuntime } from "../../api/types";

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

export type SectionId = "basics" | "runtime" | "start" | "resources" | "env" | "storage" | "health" | "advanced" | "image" | "ports" | "command" | "volumes" | "swarm";

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
  start: { id: "start", label: "Start command", icon: SquareTerminal, hint: "How systemd starts the process, and the port it listens on." },
  image: { id: "image", label: "Image", icon: Boxes, hint: "The container image to run." },
  ports: { id: "ports", label: "Ports", icon: Network, hint: "Ports published on the node: host:container or ip:host:container[/udp]." },
  command: { id: "command", label: "Command and entrypoint", icon: SquareTerminal, hint: "Overrides of the image's CMD and ENTRYPOINT, restart and stop behaviour." },
  resources: { id: "resources", label: "Resources", icon: Cpu, hint: "Limits per node. Empty means no limit." },
  env: { id: "env", label: "Environment variables", icon: Variable, hint: "Written to the service's environment file (0600). Secrets are sealed and never shown again." },
  storage: { id: "storage", label: "Storage", icon: HardDrive, hint: "Where the service keeps code and data on the node, and attached buckets." },
  volumes: { id: "volumes", label: "Volumes", icon: HardDrive, hint: "Bind mounts: /host/path:/container/path[:ro]." },
  health: { id: "health", label: "Health checks", icon: Activity, hint: "How Forge and Docker decide that the service is healthy." },
  advanced: { id: "advanced", label: "Advanced", icon: Settings2, hint: "Rarely needed options." },
  swarm: { id: "swarm", label: "Swarm", icon: Layers, hint: "Replicas, where tasks may run, and how updates and restarts roll out." },
};

export const SECTIONS: Record<EditableRuntime, SectionMeta[]> = {
  systemd: [S.basics, S.runtime, S.start, S.resources, S.env, S.storage, S.health, S.advanced],
  container: [S.basics, S.image, S.ports, S.command, S.resources, S.env, S.volumes, S.health, { ...S.advanced, hint: "User, host name, labels, log rotation, and the whole container as compose YAML." }],
};

/** The sections of an input: a container in a Swarm cluster (`swarm` set) also gets the Swarm section. */
export const sectionsFor = (v: ServiceInput): SectionMeta[] =>
  v.runtime === "container" && v.swarm ? [...SECTIONS.container.slice(0, 3), S.swarm, ...SECTIONS.container.slice(3)] : SECTIONS[v.runtime];

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
      break;
    case "image":
      if (!v.image.trim()) needs.push("Enter an image.");
      else if (/\s/.test(v.image)) errors.push("Image: no spaces.");
      break;
    case "ports":
      if (v.ports.some((p) => p && !PORT_RE.test(p))) errors.push("Ports: use host:container or ip:host:container[/udp].");
      break;
    case "command":
      if (v.stop_timeout_secs < 0 || v.stop_timeout_secs > 3600) errors.push("Stop timeout: 0 to 3600 seconds.");
      break;
    case "resources":
      if (v.cpu_millis < 0 || v.memory_mb < 0) errors.push("Limits cannot be negative.");
      break;
    case "env":
      if (v.env.some((e) => e.key && !KEY_RE.test(e.key))) errors.push("Names: letters, digits and underscores; not starting with a digit.");
      if (v.env.some((e) => !e.key && (e.value ?? "") !== "")) needs.push("A variable has a value but no name.");
      if (dupes.length) errors.push(`Duplicate names: ${[...new Set(dupes)].join(", ")}.`);
      break;
    case "volumes":
      if (v.volumes.some((p) => p && !VOL_RE.test(p))) errors.push("Volumes: use /host/path:/container/path[:ro].");
      break;
    case "health":
      if (v.runtime === "container" && v.healthcheck && !v.healthcheck.command.trim()) needs.push("Enter the health check command, or turn it off.");
      break;
    case "advanced":
      if (v.runtime === "container" && v.extra_hosts.some((h) => h && !HOST_RE.test(h))) errors.push("Extra hosts: use name:ipv4.");
      if (v.log_max_mb < 0) errors.push("Log rotation cannot be negative.");
      break;
    case "swarm": {
      const w = v.swarm;
      if (!w) break;
      if (w.mode === "replicated" && (!Number.isInteger(w.replicas) || w.replicas < 1 || w.replicas > 1000)) errors.push("Replicas: 1 to 1000.");
      if (w.constraints.some((c) => c && !CONSTRAINT_RE.test(c))) errors.push("Constraints: enter a value for each row.");
      if (w.spread.filter(Boolean).length > 4) errors.push("Spread: at most 4 labels.");
      if (w.spread.some((x) => x && !SPREAD_RE.test(x))) errors.push("Spread: use node.labels.<name>.");
      if (w.max_per_node < 0 || w.update.parallelism < 0 || w.restart.max_attempts < 0) errors.push("Counts cannot be negative.");
      if (w.update.max_failure_ratio < 0 || w.update.max_failure_ratio > 1) errors.push("Failure ratio: 0 to 1.");
      break;
    }
    case "runtime":
    case "storage":
      break;
  }
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
});

const arr = <T,>(v: T[] | undefined | null): T[] => (Array.isArray(v) ? v : []);

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
    ...(s.swarm ? { swarm: normSwarmSettings(s.swarm) } : {}),
  };
}

/** What changing the kind clears, for the confirmation. */
export const CLEARED_ON_SWITCH: Record<EditableRuntime, string> = {
  systemd: "the image, ports, volumes, entrypoint, user, host name, labels, extra hosts, Docker health check, stop timeout, read-only flag and log size",
  container: "the runtimes, working directory and listening port",
};

/** The input with the other kind's fields cleared: the server rejects container-only fields on systemd services. */
export function switchKind(v: ServiceInput, kind: EditableRuntime): ServiceInput {
  const empty = emptyInput(kind);
  if (kind === "systemd") {
    return { ...v, runtime: kind, image: "", ports: [], volumes: [], entrypoint: "", user: "", hostname: "", labels: [], extra_hosts: [], healthcheck: null, stop_timeout_secs: 0, read_only: false, log_max_mb: 0 };
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
