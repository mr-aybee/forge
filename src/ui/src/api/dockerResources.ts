// Docker images, volumes, networks, disk usage and events on one node, run by
// the node's root helper (no TLS endpoint needed). Server:
// `src/apps/forge-server/src/api/docker_resources.rs`. Every reader normalizes
// the node's JSON so an older server or a missing field never crashes a view.
import { useQuery } from "@tanstack/react-query";

import { ApiError, get, post } from "./client";
import { useAction } from "./hooks";
import type { Job } from "./types";

const enc = encodeURIComponent;
const base = (server: string) => `/api/v1/servers/${enc(server)}/docker`;

export interface DockerImage {
  /** Short id (12 hex). */
  id: string;
  full_id: string;
  /** `repo:tag`; empty for a dangling image. */
  tags: string[];
  digests: string[];
  size?: number;
  /** Unix ms. */
  created?: number;
  /** Containers using it; Docker reports -1 when it did not count. */
  containers?: number;
  dangling: boolean;
}

export interface DockerVolume {
  name: string;
  driver: string;
  mountpoint?: string;
  scope?: string;
  /** Unix ms. */
  created?: number;
  labels: Record<string, string>;
  /** Created by Forge (`forge.managed=true`). */
  managed: boolean;
  /** Containers using it; empty when the server does not say. */
  used_by: string[];
}

/** Where a volume's data lives when it is not on the node's disk. */
export type VolumeRemote =
  | { type: "nfs"; server: string; path: string; version?: string; read_only: boolean }
  | { type: "cifs"; server: string; share: string; username?: string; password?: string; read_only: boolean };

export interface DockerNetwork {
  id: string;
  name: string;
  driver: string;
  scope?: string;
  internal: boolean;
  attachable: boolean;
  subnets: string[];
  containers: number;
  /** Docker's or Forge's own network: no remove, connect or disconnect. */
  protected: boolean;
}

/** One environment variable of an image; `value` is null when hidden (no `container.inspect_env`). */
export interface ImageEnv {
  key: string;
  value: string | null;
}

/** `image_inspect` result: the typed view the server returns. */
export interface DockerImageDetail {
  id: string;
  full_id: string;
  tags: string[];
  digests: string[];
  /** Unix ms. */
  created?: number;
  size?: number;
  architecture?: string;
  os?: string;
  author?: string;
  entrypoint: string[];
  cmd: string[];
  working_dir?: string;
  user?: string;
  exposed_ports: string[];
  volumes: string[];
  labels: Record<string, string>;
  env: ImageEnv[];
  /** Layer digests, bottom first. */
  layers: string[];
}

export interface DiskBucket {
  count?: number;
  bytes?: number;
}

export interface DockerDiskUsage {
  images: DiskBucket;
  containers: DiskBucket;
  volumes: DiskBucket;
  build_cache: DiskBucket;
}

export interface DockerEvent {
  /** Unix ms. */
  time?: number;
  type: string;
  action: string;
  id?: string;
  name?: string;
}

export type NetworkDriver = "bridge" | "overlay";

/** Body of `POST /servers/{id}/docker/ops`. */
export type DockerOp =
  | { op: "image_inspect"; id: string }
  | { op: "image_history"; id: string }
  | { op: "image_remove"; id: string; force: boolean }
  | { op: "image_tag"; id: string; repo: string; tag: string }
  | { op: "image_pull"; image: string }
  | { op: "image_push"; image: string }
  | { op: "image_prune"; all: boolean }
  | { op: "volume_inspect"; name: string }
  | { op: "volume_create"; name: string; labels: Record<string, string>; remote?: VolumeRemote }
  | { op: "volume_remove"; name: string; force: boolean }
  | { op: "volume_prune" }
  | { op: "network_inspect"; id: string }
  | {
      op: "network_create";
      name: string;
      driver: NetworkDriver;
      subnet?: string;
      gateway?: string;
      internal: boolean;
      attachable: boolean;
      labels: Record<string, string>;
    }
  | { op: "network_remove"; id: string }
  | { op: "network_connect"; network: string; container: string }
  | { op: "network_disconnect"; network: string; container: string; force: boolean }
  | { op: "network_prune" };

export type DockerInspectOp = Extract<DockerOp, { op: "image_inspect" | "image_history" | "volume_inspect" | "network_inspect" }>;

/** Operations the server runs as a `docker.op` job (`202`). */
const JOB_OPS: ReadonlySet<DockerOp["op"]> = new Set(["image_pull", "image_push", "image_prune", "volume_prune", "network_prune"]);

/** A finished quick operation, or a started job. */
export type DockerOpOutcome = { kind: "done"; result: unknown } | { kind: "job"; job: Job };

export const EVENT_WINDOWS = [300, 3600, 86400] as const;
export type EventWindow = (typeof EVENT_WINDOWS)[number];

export const dockerResKeys = {
  all: (server: string) => ["docker-res", server] as const,
  images: (server: string) => ["docker-res", server, "images"] as const,
  volumes: (server: string) => ["docker-res", server, "volumes"] as const,
  networks: (server: string) => ["docker-res", server, "networks"] as const,
  disk: (server: string) => ["docker-res", server, "disk"] as const,
  events: (server: string, since: number) => ["docker-res", server, "events", since] as const,
  inspect: (server: string, op: DockerInspectOp) => ["docker-res", server, "inspect", op] as const,
};

// ------------------------------------------------------------------ normalizers

type Obj = Record<string, unknown>;
const obj = (v: unknown): Obj => (v && typeof v === "object" && !Array.isArray(v) ? (v as Obj) : {});
const arr = (v: unknown): unknown[] => (Array.isArray(v) ? v : []);
const str = (v: unknown) => (typeof v === "string" ? v : "");
const num = (v: unknown) => (typeof v === "number" && Number.isFinite(v) ? v : undefined);
const strs = (v: unknown) => arr(v).filter((x): x is string => typeof x === "string" && x !== "" && x !== "<none>:<none>" && x !== "<none>@<none>");
const labels = (v: unknown): Record<string, string> =>
  Object.fromEntries(Object.entries(obj(v)).filter((e): e is [string, string] => typeof e[1] === "string"));
/** Unix seconds or an ISO date as unix ms. */
function ms(v: unknown): number | undefined {
  const n = num(v);
  if (n !== undefined) return n > 0 ? (n < 1e12 ? n * 1000 : n) : undefined;
  if (typeof v === "string" && v) {
    const t = Date.parse(v);
    return Number.isNaN(t) ? undefined : t;
  }
  return undefined;
}

export function normImage(v: unknown): DockerImage {
  const o = obj(v);
  const tags = strs(o.tags);
  return {
    id: str(o.id) || str(o.full_id).replace(/^sha256:/, "").slice(0, 12),
    full_id: str(o.full_id) || str(o.id),
    tags,
    digests: strs(o.digests),
    size: num(o.size),
    created: ms(o.created),
    containers: num(o.containers),
    dangling: typeof o.dangling === "boolean" ? o.dangling : tags.length === 0,
  };
}

export function normImageDetail(v: unknown): DockerImageDetail {
  const o = obj(v);
  const base = normImage(v);
  return {
    id: base.id,
    full_id: base.full_id,
    tags: base.tags,
    digests: base.digests,
    created: base.created,
    size: base.size,
    architecture: str(o.architecture) || undefined,
    os: str(o.os) || undefined,
    author: str(o.author) || undefined,
    entrypoint: arr(o.entrypoint).filter((x): x is string => typeof x === "string"),
    cmd: arr(o.cmd).filter((x): x is string => typeof x === "string"),
    working_dir: str(o.working_dir) || undefined,
    user: str(o.user) || undefined,
    exposed_ports: strs(o.exposed_ports),
    volumes: strs(o.volumes),
    labels: labels(o.labels),
    env: arr(o.env)
      .map(obj)
      .filter((e) => typeof e.key === "string" && e.key !== "")
      .map((e) => ({ key: str(e.key), value: typeof e.value === "string" ? e.value : null })),
    layers: strs(o.layers),
  };
}

export function normVolume(v: unknown): DockerVolume {
  const o = obj(v);
  const l = labels(o.labels);
  return {
    name: str(o.name),
    driver: str(o.driver) || "local",
    mountpoint: str(o.mountpoint) || undefined,
    scope: str(o.scope) || undefined,
    created: ms(o.created),
    labels: l,
    managed: o.managed === true || l["forge.managed"] === "true",
    used_by: strs(o.used_by),
  };
}

export function normNetwork(v: unknown): DockerNetwork {
  const o = obj(v);
  return {
    id: str(o.id),
    name: str(o.name),
    driver: str(o.driver),
    scope: str(o.scope) || undefined,
    internal: o.internal === true,
    attachable: o.attachable === true,
    subnets: strs(o.subnets),
    containers: num(o.containers) ?? 0,
    protected: o.protected === true,
  };
}

const bucket = (v: unknown): DiskBucket => ({ count: num(obj(v).count), bytes: num(obj(v).bytes) });

export function normDisk(v: unknown): DockerDiskUsage {
  const o = obj(v);
  return { images: bucket(o.images), containers: bucket(o.containers), volumes: bucket(o.volumes), build_cache: bucket(o.build_cache) };
}

export function normEvent(v: unknown): DockerEvent {
  const o = obj(v);
  return { time: ms(o.time), type: str(o.type) || "—", action: str(o.action) || "—", id: str(o.id) || undefined, name: str(o.name) || undefined };
}

/** True when the node answered that Docker Engine is missing or its socket is not reachable. */
export function isDockerUnavailable(e: unknown): boolean {
  const m = e instanceof Error ? e.message : String(e ?? "");
  return /docker is not (available|reachable|installed)|docker\.sock|cannot connect to the docker daemon/i.test(m);
}

/** True when this server has no Docker resource endpoints (older version). */
export const isMissingEndpoint = (e: unknown) => e instanceof ApiError && (e.status === 404 || e.status === 405);

// ------------------------------------------------------------------ queries

const list = <T>(server: string, path: string, norm: (v: unknown) => T) => get<unknown>(`${base(server)}/${path}`).then((v) => arr(v).map(norm));

const common = { retry: false, staleTime: 10_000 } as const;

export const useDockerImages = (server: string, enabled = true) =>
  useQuery({ queryKey: dockerResKeys.images(server), queryFn: () => list(server, "images", normImage), enabled: enabled && !!server, ...common });

export const useDockerVolumes = (server: string, enabled = true) =>
  useQuery({ queryKey: dockerResKeys.volumes(server), queryFn: () => list(server, "volumes", normVolume), enabled: enabled && !!server, ...common });

export const useDockerNetworks = (server: string, enabled = true) =>
  useQuery({ queryKey: dockerResKeys.networks(server), queryFn: () => list(server, "networks", normNetwork), enabled: enabled && !!server, ...common });

export const useDockerDiskUsage = (server: string, enabled = true) =>
  useQuery({
    queryKey: dockerResKeys.disk(server),
    queryFn: () => get<unknown>(`${base(server)}/disk-usage`).then(normDisk),
    enabled: enabled && !!server,
    ...common,
    staleTime: 30_000,
  });

/** Recent events, newest first; refreshes every 15 s while shown. */
export const useDockerEvents = (server: string, since: EventWindow, enabled = true) =>
  useQuery({
    queryKey: dockerResKeys.events(server, since),
    queryFn: () =>
      list(server, `events?since_secs=${since}`, normEvent).then((ev) => [...ev].sort((a, b) => (b.time ?? 0) - (a.time ?? 0))),
    enabled: enabled && !!server,
    ...common,
    refetchInterval: 15_000,
  });

/** Inspect or history of one object: the node's JSON as it is. Never cached across opens. */
export const useDockerInspect = (server: string, op: DockerInspectOp) =>
  useQuery({
    queryKey: dockerResKeys.inspect(server, op),
    queryFn: () => post<unknown>(`${base(server)}/ops`, op),
    retry: false,
    gcTime: 0,
  });

// ------------------------------------------------------------------ mutations

/** Runs one operation; pulls, pushes and prunes come back as a started job. */
export const useDockerOp = (server: string) =>
  useAction(
    (op: DockerOp): Promise<DockerOpOutcome> =>
      post<unknown>(`${base(server)}/ops`, op).then((r) => {
        const j = obj(r);
        return JOB_OPS.has(op.op) && typeof j.id === "string" ? { kind: "job", job: r as Job } : { kind: "done", result: r };
      }),
    [dockerResKeys.all(server), ["containers", server], ["jobs"]],
  );
