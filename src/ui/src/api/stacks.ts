// Compose stacks and single containers (issue #7). Every reader normalizes
// the server's JSON so an older server, a missing field or a failed request
// never crashes a view.
import {
  useMutation,
  useQuery,
  useQueryClient,
  type QueryKey,
} from "@tanstack/react-query";

import { ApiError, api, get, post } from "./client";
import type { Job } from "./types";

const enc = encodeURIComponent;

// ------------------------------------------------------------------ types

export type StackKind = "compose" | "container";
export type StackNodeStatus =
  | "pending"
  | "deploying"
  | "deployed"
  | "failed"
  | "removing"
  | "stopping"
  | "starting"
  | "pulling"
  | string;
export type Policy = "allowed" | "privileged" | "blocked";
export type Severity = "blocked" | "privileged" | "info";

export interface Stack {
  id: string;
  workspace_id: string;
  environment_id: string;
  name: string;
  kind: StackKind;
  default_server: string;
  dir: string | null;
  revision: number;
  deployed_revision: number;
  env_keys: string[];
  created_by: string;
  created_at: number;
  updated_at: number;
}

export interface ServiceState {
  service: string;
  name: string;
  state: string;
  health: string;
  status: string;
  image: string;
  exit_code: number | null;
  ports: string;
  /** Address on the cluster container network; absent when not attached or on older servers. */
  mesh_ip?: string;
}

export interface StackNode {
  stack_id: string;
  server_id: string;
  services: string[];
  selected: string[];
  revision: number;
  status: StackNodeStatus;
  state: ServiceState[];
  last_job_id: string | null;
  last_error: string | null;
  updated_at: number;
}

export interface StackEntry {
  stack: Stack;
  nodes: StackNode[];
}

export interface Finding {
  service: string;
  severity: Severity;
  message: string;
}

export interface PreviewPort {
  service: string;
  host_ip: string;
  published: string;
  target: string;
  protocol: string;
  public: boolean;
}

export interface PreviewService {
  name: string;
  image: string;
  command: string;
  entrypoint: string;
  ports: PreviewPort[];
  volumes: string[];
  environment: { key: string; value: string }[];
  env_file: string;
  depends_on: string[];
  healthcheck: { test: string; interval: string } | null;
  restart: string;
  expose: "http" | "https" | null;
  nodes: { server_id: string; hostname: string }[];
  policy: Policy;
  findings: Finding[];
  /** Joins the cluster container network: true, false or "off (<reason>)". Absent on older servers. */
  mesh?: boolean | string;
}

export interface PreviewNode {
  server_id: string;
  hostname: string;
  services: string[];
  internal_ip: string;
  ports: PreviewPort[];
  findings: Finding[];
}

export interface Preview {
  services: PreviewService[];
  nodes: PreviewNode[];
  findings: Finding[];
  privileged: boolean;
  blocked: boolean;
  /** Only from `/stacks/preview`: whether the caller may deploy privileged stacks. */
  can_privileged?: boolean;
}

export interface StackDetail {
  stack: Stack;
  content: string;
  dir: string;
  nodes: StackNode[];
  preview: Preview | null;
  preview_error?: string;
}

export interface Revision {
  stack_id: string;
  revision: number;
  sha256: string;
  note: string | null;
  author: string;
  created_at: number;
  content?: string;
}

export interface ContainerForm {
  name: string;
  image: string;
  command?: string;
  entrypoint?: string;
  ports: {
    container: number;
    host?: number;
    protocol: "tcp" | "udp";
    public: boolean;
  }[];
  env: { key: string; value: string }[];
  volumes: { source: string; target: string; read_only: boolean }[];
  restart: "no" | "always" | "on-failure" | "unless-stopped";
  cpus?: number;
  memory_mb?: number;
  network?: string;
  healthcheck?: { test: string; interval: string };
  labels?: Record<string, string>;
  expose?: "http" | "https";
}

export interface TemplateVariable {
  name: string;
  label: string;
  default: string;
  /** Rendered as a password input; empty means the server generates a value. */
  secret: boolean;
}

export interface StackTemplate {
  id: string;
  title: string;
  description: string;
  category: string;
  compose: string;
  variables: TemplateVariable[];
}

/** Query keys for stack data that is not under `["stacks"]`. */
export const stackKeys = {
  templates: () => ["stack-templates"] as const,
};

// ------------------------------------------------------------------ normalizers

type Obj = Record<string, unknown>;
const isObj = (v: unknown): v is Obj =>
  !!v && typeof v === "object" && !Array.isArray(v);
const s = (v: unknown, d = ""): string =>
  typeof v === "string" ? v : typeof v === "number" ? String(v) : d;
const n = (v: unknown, d = 0): number =>
  typeof v === "number" && Number.isFinite(v) ? v : d;
const arr = (v: unknown): unknown[] => (Array.isArray(v) ? v : []);
const strs = (v: unknown): string[] =>
  arr(v).filter((x): x is string => typeof x === "string");

/** A compose value (string, list or map) as one readable line. */
export function flat(v: unknown): string {
  if (v === null || v === undefined) return "";
  if (typeof v === "string") return v;
  if (typeof v === "number" || typeof v === "boolean") return String(v);
  if (Array.isArray(v)) return v.map(flat).join(" ");
  try {
    return JSON.stringify(v);
  } catch {
    return "";
  }
}

/** A principal (`{type, id}`, a string, or missing) as a short label. */
function principal(v: unknown): string {
  if (typeof v === "string") return v;
  if (isObj(v))
    return [s(v.type), s(v.id).slice(0, 8)].filter(Boolean).join(":");
  return "";
}

export function normStack(v: unknown): Stack | null {
  if (!isObj(v) || typeof v.id !== "string") return null;
  return {
    id: v.id,
    workspace_id: s(v.workspace_id),
    environment_id: s(v.environment_id),
    name: s(v.name, v.id.slice(0, 8)),
    kind: v.kind === "container" ? "container" : "compose",
    default_server: s(v.default_server),
    dir: typeof v.dir === "string" && v.dir ? v.dir : null,
    revision: n(v.revision),
    deployed_revision: n(v.deployed_revision),
    env_keys: strs(v.env_keys),
    created_by: principal(v.created_by),
    created_at: n(v.created_at),
    updated_at: n(v.updated_at),
  };
}

function normState(v: unknown): ServiceState[] {
  return arr(v).flatMap((x): ServiceState[] => {
    if (!isObj(x)) return [];
    return [
      {
        service: s(x.service),
        name: s(x.name),
        state: s(x.state),
        health: s(x.health),
        status: s(x.status),
        image: s(x.image),
        exit_code: typeof x.exit_code === "number" ? x.exit_code : null,
        ports: flat(x.ports),
        mesh_ip: typeof x.mesh_ip === "string" && x.mesh_ip ? x.mesh_ip : undefined,
      },
    ];
  });
}

export function normNode(v: unknown): StackNode | null {
  if (!isObj(v) || typeof v.server_id !== "string") return null;
  return {
    stack_id: s(v.stack_id),
    server_id: v.server_id,
    services: strs(v.services),
    selected: strs(v.selected),
    revision: n(v.revision),
    status: s(v.status, "pending"),
    state: normState(v.state),
    last_job_id: typeof v.last_job_id === "string" ? v.last_job_id : null,
    last_error:
      typeof v.last_error === "string" && v.last_error ? v.last_error : null,
    updated_at: n(v.updated_at),
  };
}

const normNodes = (v: unknown) =>
  arr(v)
    .map(normNode)
    .filter((x): x is StackNode => !!x);

function normFinding(v: unknown): Finding | null {
  if (!isObj(v)) return null;
  const sev =
    v.severity === "blocked" || v.severity === "privileged"
      ? v.severity
      : "info";
  return { service: s(v.service), severity: sev, message: s(v.message) };
}
const normFindings = (v: unknown) =>
  arr(v)
    .map(normFinding)
    .filter((x): x is Finding => !!x);

function normPort(v: unknown): PreviewPort | null {
  if (!isObj(v)) return null;
  return {
    service: s(v.service),
    host_ip: s(v.host_ip),
    published: s(v.published),
    target: s(v.target),
    protocol: s(v.protocol, "tcp"),
    public: v.public === true,
  };
}
const normPorts = (v: unknown) =>
  arr(v)
    .map(normPort)
    .filter((x): x is PreviewPort => !!x);

export function normPreview(v: unknown): Preview | null {
  if (!isObj(v) || !Array.isArray(v.services)) return null;
  const services = v.services.flatMap((x): PreviewService[] => {
    if (!isObj(x) || typeof x.name !== "string") return [];
    const hc = isObj(x.healthcheck)
      ? {
          test: flat(x.healthcheck.test),
          interval: flat(x.healthcheck.interval),
        }
      : null;
    return [
      {
        name: x.name,
        image: flat(x.image),
        command: flat(x.command),
        entrypoint: flat(x.entrypoint),
        ports: normPorts(x.ports),
        volumes: arr(x.volumes).map(flat).filter(Boolean),
        environment: arr(x.environment).flatMap((e) =>
          isObj(e) ? [{ key: s(e.key), value: flat(e.value) }] : [],
        ),
        env_file: flat(x.env_file),
        depends_on: strs(x.depends_on),
        healthcheck: hc,
        restart: flat(x.restart),
        expose: x.expose === "http" || x.expose === "https" ? x.expose : null,
        nodes: arr(x.nodes).flatMap((m) =>
          isObj(m)
            ? [
                {
                  server_id: s(m.server_id),
                  hostname: s(m.hostname, s(m.server_id).slice(0, 8)),
                },
              ]
            : [],
        ),
        policy:
          x.policy === "blocked" || x.policy === "privileged"
            ? x.policy
            : "allowed",
        findings: normFindings(x.findings),
        mesh: typeof x.mesh === "boolean" || typeof x.mesh === "string" ? x.mesh : undefined,
      },
    ];
  });
  const nodes = arr(v.nodes).flatMap((x): PreviewNode[] => {
    if (!isObj(x)) return [];
    return [
      {
        server_id: s(x.server_id),
        hostname: s(x.hostname, s(x.server_id).slice(0, 8)),
        services: strs(x.services),
        internal_ip: s(x.internal_ip),
        ports: normPorts(x.ports),
        findings: normFindings(x.findings),
      },
    ];
  });
  return {
    services,
    nodes,
    findings: normFindings(v.findings),
    privileged: v.privileged === true,
    blocked: v.blocked === true || services.some((x) => x.policy === "blocked"),
    can_privileged:
      typeof v.can_privileged === "boolean" ? v.can_privileged : undefined,
  };
}

function normRevision(v: unknown): Revision | null {
  if (!isObj(v) || typeof v.revision !== "number") return null;
  return {
    stack_id: s(v.stack_id),
    revision: v.revision,
    sha256: s(v.sha256),
    note: typeof v.note === "string" && v.note ? v.note : null,
    author: principal(v.author),
    created_at: n(v.created_at),
    content: typeof v.content === "string" ? v.content : undefined,
  };
}

const jobsOf = (v: unknown): Job[] =>
  (Array.isArray(v) ? v : [v]).filter(
    (j): j is Job => isObj(j) && typeof j.id === "string",
  );

/** The node a stack job runs on (from its payload). */
export const jobServer = (j: Job): string =>
  isObj(j.payload) ? s(j.payload.server_id) : "";

// ------------------------------------------------------------------ queries

const busy = (nodes: StackNode[]) => nodes.some((x) => /ing$/.test(x.status));

/** Stacks, optionally of one environment or touching one node. `[]` on an older server. */
export const useStacks = (
  filter: { environment?: string; server?: string } = {},
  enabled = true,
) =>
  useQuery({
    queryKey: ["stacks", filter.environment ?? "", filter.server ?? ""],
    enabled,
    queryFn: async (): Promise<StackEntry[]> => {
      const q = new URLSearchParams();
      if (filter.environment) q.set("environment_id", filter.environment);
      if (filter.server) q.set("server_id", filter.server);
      const qs = q.toString();
      const r = await get<unknown>(`/api/v1/stacks${qs ? `?${qs}` : ""}`);
      return arr(r).flatMap((x): StackEntry[] => {
        if (!isObj(x)) return [];
        const stack = normStack(x.stack);
        return stack ? [{ stack, nodes: normNodes(x.nodes) }] : [];
      });
    },
    retry: (count, e) =>
      !(e instanceof ApiError && (e.status === 404 || e.status === 403)) &&
      count < 2,
    refetchInterval: (q) =>
      busy((q.state.data ?? []).flatMap((e) => e.nodes)) ? 3000 : 30_000,
    staleTime: 5_000,
  });

function normTemplate(v: unknown): StackTemplate | null {
  if (!isObj(v) || !s(v.id)) return null;
  return {
    id: s(v.id),
    title: s(v.title) || s(v.id),
    description: s(v.description),
    category: s(v.category) || "Other",
    compose: s(v.compose),
    variables: arr(v.variables).flatMap((x): TemplateVariable[] =>
      isObj(x) && s(x.name)
        ? [
            {
              name: s(x.name),
              label: s(x.label) || s(x.name),
              default: s(x.default),
              secret: x.secret === true,
            },
          ]
        : [],
    ),
  };
}

/** Compose app templates. `[]` on an older server without the endpoint. */
export const useStackTemplates = () =>
  useQuery({
    queryKey: stackKeys.templates(),
    queryFn: async (): Promise<StackTemplate[]> => {
      try {
        return arr(await get<unknown>("/api/v1/stack-templates"))
          .map(normTemplate)
          .filter((x): x is StackTemplate => !!x);
      } catch (e) {
        if (e instanceof ApiError && e.status === 404) return [];
        throw e;
      }
    },
    retry: (count, e) =>
      !(e instanceof ApiError && e.status < 500) && count < 2,
    staleTime: 5 * 60_000,
  });

export const useStack = (id?: string) =>
  useQuery({
    queryKey: ["stack", id],
    enabled: !!id,
    queryFn: async (): Promise<StackDetail> => {
      const r = await get<unknown>(`/api/v1/stacks/${enc(id ?? "")}`);
      const o = isObj(r) ? r : {};
      const stack = normStack(o.stack);
      if (!stack) throw new Error("The server returned no stack.");
      const pv = normPreview(o.preview);
      return {
        stack,
        content: s(o.content),
        dir: s(o.dir, stack.dir ?? ""),
        nodes: normNodes(o.nodes),
        preview: pv,
        preview_error:
          !pv && isObj(o.preview) ? s(o.preview.error) || undefined : undefined,
      };
    },
    retry: (count, e) =>
      !(e instanceof ApiError && e.status < 500) && count < 2,
    refetchInterval: (q) =>
      q.state.data && busy(q.state.data.nodes) ? 2500 : 30_000,
  });

export const useRevisions = (id?: string, enabled = true) =>
  useQuery({
    queryKey: ["stack", id, "revisions"],
    enabled: !!id && enabled,
    queryFn: async () =>
      arr(await get<unknown>(`/api/v1/stacks/${enc(id ?? "")}/revisions`))
        .map(normRevision)
        .filter((x): x is Revision => !!x),
  });

export const useRevision = (id?: string, rev?: number) =>
  useQuery({
    queryKey: ["stack", id, "revision", rev],
    enabled: !!id && !!rev,
    staleTime: Infinity,
    queryFn: async () =>
      normRevision(
        await get<unknown>(`/api/v1/stacks/${enc(id ?? "")}/revisions/${rev}`),
      ),
  });

export async function previewStack(b: {
  environment_id: string;
  server_id: string;
  name?: string;
  content: string;
  env?: string;
  dir?: string;
}): Promise<Preview> {
  const p = normPreview(await post<unknown>("/api/v1/stacks/preview", b));
  if (!p) throw new Error("The server returned no preview.");
  return p;
}

/** Stack log tail for one service. Waits for the job when the quick wait ran out. */
export async function stackLogs(
  id: string,
  server: string,
  service: string,
  tail: number,
  signal?: { stop: boolean },
): Promise<string> {
  const r = await get<unknown>(
    `/api/v1/stacks/${enc(id)}/logs?server_id=${enc(server)}&service=${enc(service)}&tail=${tail}`,
  );
  const o = isObj(r) ? r : {};
  if (o.status !== "running") return flat(o.logs);
  const job = s(o.job_id);
  const deadline = Date.now() + 120_000;
  while (job && Date.now() < deadline && !signal?.stop) {
    await new Promise((res) => setTimeout(res, 1500));
    const j = await get<Job>(`/api/v1/jobs/${enc(job)}`);
    if (j?.status === "succeeded") {
      const out =
        isObj(j.state) && isObj(j.state.output)
          ? j.state.output.logs
          : undefined;
      return flat(out);
    }
    if (j?.status === "failed" || j?.status === "cancelled")
      throw new Error(j.last_error || "Reading the logs failed.");
  }
  throw new Error("The node did not answer in time.");
}

// ------------------------------------------------------------------ mutations

function useAction<A, R>(
  fn: (a: A) => Promise<R>,
  keys: QueryKey[] = [["stacks"], ["stack"], ["jobs"], ["services"]],
) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: fn,
    onSuccess: async () => {
      await Promise.all(keys.map((k) => qc.invalidateQueries({ queryKey: k })));
    },
  });
}

export const useCreateStack = () =>
  useAction(
    async (b: {
      environment_id: string;
      server_id: string;
      name: string;
      content: string;
      env?: string;
      dir?: string;
      note?: string;
    }) => {
      const r = await post<unknown>("/api/v1/stacks", b);
      const o = isObj(r) ? r : {};
      const stack = normStack(o.stack);
      if (!stack) throw new Error("The server returned no stack.");
      return { stack, preview: normPreview(o.preview) };
    },
  );

export const useCreateFromTemplate = () =>
  useAction(
    async (b: {
      template: string;
      environment_id: string;
      server_id: string;
      name: string;
      values: Record<string, string>;
    }) => {
      const r = await post<unknown>("/api/v1/stacks/from-template", b);
      const o = isObj(r) ? r : {};
      const stack = normStack(o.stack);
      if (!stack) throw new Error("The server returned no stack.");
      return { stack, preview: normPreview(o.preview) };
    },
  );

export const useCreateContainer = () =>
  useAction(
    async (b: {
      environment_id: string;
      server_id: string;
      dir?: string;
      start: boolean;
      container: ContainerForm;
    }) => {
      const r = await post<unknown>("/api/v1/stacks/containers", b);
      const o = isObj(r) ? r : {};
      const stack = normStack(o.stack);
      if (!stack) throw new Error("The server returned no stack.");
      return { stack, preview: normPreview(o.preview), jobs: jobsOf(o.jobs) };
    },
  );

export const useUpdateStack = () =>
  // `version`: the stack's `updated_at` when it was loaded; the server answers
  // 409 `stale_version` when someone saved or changed it since.
  useAction(
    async ({
      id,
      ...b
    }: {
      id: string;
      content?: string;
      env?: string;
      dir?: string;
      note?: string;
      version?: number;
    }) => {
      const r = await api<unknown>("PUT", `/api/v1/stacks/${enc(id)}`, b);
      const o = isObj(r) ? r : {};
      return { stack: normStack(o.stack), preview: normPreview(o.preview) };
    },
  );

export const useDeployStack = () =>
  useAction(
    async ({
      id,
      ...b
    }: {
      id: string;
      revision?: number;
      services?: string[];
      pull?: boolean;
    }) => jobsOf(await post<unknown>(`/api/v1/stacks/${enc(id)}/deploy`, b)),
  );

export const useStackServiceAction = () =>
  useAction(
    async ({
      id,
      ...b
    }: {
      id: string;
      server_id: string;
      action: "start" | "stop" | "restart" | "pull";
      services?: string[];
    }) => jobsOf(await post<unknown>(`/api/v1/stacks/${enc(id)}/services`, b)),
  );

export const useRefreshStack = () =>
  useAction(async (id: string) =>
    jobsOf(await post<unknown>(`/api/v1/stacks/${enc(id)}/refresh`, {})),
  );

export const useDownStack = () =>
  useAction(
    async ({
      id,
      ...b
    }: {
      id: string;
      volumes: boolean;
      server_id?: string;
    }) => jobsOf(await post<unknown>(`/api/v1/stacks/${enc(id)}/down`, b)),
  );

export const useDeleteStack = () =>
  useAction((id: string) => api<void>("DELETE", `/api/v1/stacks/${enc(id)}`));

export const useExposeStack = () =>
  useAction(
    async ({
      id,
      ...b
    }: {
      id: string;
      service: string;
      port: number;
      domains: string[];
      tls?: "off" | "acme" | "uploaded";
      acme_email?: string;
      cert_id?: string;
      edges: string[];
    }) => {
      const r = await post<unknown>(`/api/v1/stacks/${enc(id)}/expose`, b);
      const o = isObj(r) ? r : {};
      return { site: o.site, next: s(o.next) };
    },
    [["stacks"], ["stack"], ["sites"], ["ingress"]],
  );

// ------------------------------------------------------------------ helpers

/** Every service the ticked ones pull in through `depends_on` (transitively). */
export function withDeps(
  ticked: Iterable<string>,
  services: PreviewService[],
): Set<string> {
  const by = new Map(services.map((x) => [x.name, x]));
  const out = new Set<string>();
  const stack = [...ticked];
  while (stack.length) {
    const name = stack.pop() as string;
    if (out.has(name)) continue;
    out.add(name);
    for (const d of by.get(name)?.depends_on ?? [])
      if (!out.has(d)) stack.push(d);
  }
  return out;
}

/** True when the error means "verify with MFA first". */
export const needsMfa = (e: unknown) =>
  e instanceof ApiError &&
  e.status === 403 &&
  (e.code === "mfa_required" || e.code === "step_up_required");

export const STACK_NAME = /^[a-z0-9][a-z0-9_-]{0,62}$/;
