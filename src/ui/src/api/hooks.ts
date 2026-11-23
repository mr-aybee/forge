import { useEffect, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient, type QueryKey } from "@tanstack/react-query";

import { ApiError, api, get, post } from "./client";
import type {
  ServiceHealthView,
  ServiceInput,
  ServiceInstance,
  ServiceStorageAttach,
  ServiceStorageAttachment,
  ServiceStorageView,
  HealthPolicy,
  HealthValidation,
  HealthWarning,
  InstanceCheck,
  InstanceHealth,
  Service,
  Capabilities,
  Catalog,
  ContainerEnv,
  ContainerInspect,
  ContainerStats,
  ContainerMount,
  ContainerPort,
  Installation,
  PackageRelease,
  UpdatePolicy,
  UpdateRow,
  Cluster,
  Environment,
  ProjectBase,
  Project,
  ServiceSpec,
  HostKey,
  ConsoleLine,
  Job,
  JobStep,
  LiveEvent,
  OnboardingConsole,
  OnboardingNode,
  Me,
  Plugin,
  RoleBinding,
  SecretMeta,
  LocalNames,
  NameSuffix,
  NodeDns,
  Server,
  TagCount,
  Tenant,
  User,
  VersionInfo,
  Workspace,
  ServicePort,
  Tunnel,
  TunnelGrant,
  TunnelTarget,
} from "./types";

const enc = encodeURIComponent;

// ------------------------------------------------------------------ queries

// Polls only while signed in. Signed out, a refetch with no data puts the query
// back into its loading state, which made the gate remount the login form
// (losing what was typed) every 30 s and on every window focus.
export const useMe = () =>
  useQuery({
    queryKey: ["me"],
    queryFn: () => get<Me>("/api/v1/me"),
    retry: false,
    refetchInterval: (q) => (q.state.data ? 30_000 : false),
    refetchOnWindowFocus: (q) => q.state.data !== undefined,
  });
export const useCapabilities = () =>
  useQuery({ queryKey: ["caps"], queryFn: () => get<Capabilities>("/api/capabilities") });
export const useVersion = () => useQuery({ queryKey: ["version"], queryFn: () => get<VersionInfo>("/api/version") });
/** The deployment's single, implicit workspace (one deployment = one workspace). */
export const useWorkspace = () =>
  useQuery({ queryKey: ["workspace"], queryFn: () => get<Workspace>("/api/v1/workspace"), retry: false });

/**
 * Query keys of projects, environments and services. Mutations invalidate by
 * these prefixes, so every list and detail of a kind refreshes together.
 */
export const fleetKeys = {
  projects: ["projects"] as const,
  project: (id: string | undefined) => ["projects", id] as const,
  environments: ["environments"] as const,
  services: ["services"] as const,
  serviceList: (environment: string | undefined, cluster: string | null | undefined) => ["services", environment ?? "all", cluster ?? "any"] as const,
  service: (id: string | undefined) => ["service", id] as const,
  serviceAny: ["service"] as const,
  storage: (id: string | undefined) => ["service-storage", id] as const,
  health: (id: string | undefined) => ["service-health", id] as const,
};

const optId = (v: unknown): string | null => (typeof v === "string" && v ? v : null);

/** Normalizes one `/environments` row; `[]` when it is unusable. */
function toEnvironment(x: unknown): Environment[] {
  if (!x || typeof x !== "object") return [];
  const o = x as Record<string, unknown>;
  if (typeof o.id !== "string" || typeof o.name !== "string") return [];
  return [
    {
      id: o.id,
      name: o.name,
      workspace_id: typeof o.workspace_id === "string" ? o.workspace_id : "",
      cluster_id: optId(o.cluster_id),
      project_id: optId(o.project_id),
      protected: o.protected === true,
      created_at: typeof o.created_at === "number" ? o.created_at : 0,
    },
  ];
}

/**
 * Every environment the caller can view, across all clusters and projects.
 * Always an array of normalized items.
 */
export const useAllEnvironments = () =>
  useQuery({
    queryKey: fleetKeys.environments,
    queryFn: async (): Promise<Environment[]> => {
      const r = await get<unknown>(`/api/v1/environments`);
      return Array.isArray(r) ? r.flatMap(toEnvironment) : [];
    },
    staleTime: 15_000,
  });

/** Normalizes one project view; `[]` when unusable. */
function toProject(x: unknown): Project[] {
  if (!x || typeof x !== "object") return [];
  const o = x as Record<string, unknown>;
  if (typeof o.id !== "string" || typeof o.name !== "string") return [];
  const envs = Array.isArray(o.environments) ? o.environments : [];
  return [
    {
      id: o.id,
      name: o.name,
      workspace_id: typeof o.workspace_id === "string" ? o.workspace_id : "",
      description: typeof o.description === "string" ? o.description : "",
      created_at: typeof o.created_at === "number" ? o.created_at : 0,
      service_count: typeof o.service_count === "number" ? o.service_count : 0,
      environments: envs.flatMap((e) => {
        if (!e || typeof e !== "object") return [];
        const n = e as Record<string, unknown>;
        if (typeof n.id !== "string" || typeof n.name !== "string") return [];
        return [{ id: n.id, name: n.name, cluster_id: optId(n.cluster_id), protected: n.protected === true }];
      }),
    },
  ];
}

/** Every project with its environments and service count. Always an array of normalized items; failures surface as errors. */
export const useProjects = (enabled = true) =>
  useQuery({
    queryKey: fleetKeys.projects,
    enabled,
    queryFn: (): Promise<Project[]> => get<unknown>(`/api/v1/projects`).then((r) => (Array.isArray(r) ? r.flatMap(toProject) : [])),
    staleTime: 15_000,
  });

/** One project; `null` when it does not exist. Other failures surface as errors. */
export const useProject = (id: string | undefined) =>
  useQuery({
    queryKey: fleetKeys.project(id),
    enabled: !!id,
    queryFn: (): Promise<Project | null> =>
      get<unknown>(`/api/v1/projects/${enc(id ?? "")}`)
        .then((r) => toProject(r)[0] ?? null)
        .catch((e: unknown) => {
          if (e instanceof ApiError && e.status === 404) return null;
          throw e;
        }),
    staleTime: 15_000,
  });

/** Tenants of the workspace (role binding scopes); `[]` when unavailable. */
export const useTenants = () =>
  useQuery({
    queryKey: ["tenants"],
    queryFn: (): Promise<Tenant[]> =>
      get<unknown>(`/api/v1/tenants`)
        .then((r) => (Array.isArray(r) ? (r as Tenant[]).filter((t) => !!t && typeof t.id === "string" && typeof t.name === "string") : []))
        .catch(() => []),
    staleTime: 60_000,
  });

export const useClusters = () =>
  useQuery({ queryKey: ["clusters"], queryFn: () => get<Cluster[]>(`/api/v1/clusters`) });

export const useServers = (enabled = true) =>
  useQuery({ queryKey: ["servers"], enabled, queryFn: () => get<Server[]>(`/api/v1/servers`) });

export const useServer = (id: string) =>
  useQuery({ queryKey: ["server", id], queryFn: () => get<Server>(`/api/v1/servers/${id}`) });

/**
 * Tags in use for a kind, with counts (filters). Always an array: `[]` when
 * the server is older or the request fails. Under `["tags", kind]`, so tag
 * edits (which invalidate that prefix) refresh it.
 */
export const useTagCounts = (kind: "server" | "cluster" | "environment" | "service") =>
  useQuery({
    queryKey: ["tags", kind, "counts"],
    queryFn: (): Promise<TagCount[]> =>
      get<unknown>(`/api/v1/tags?kind=${kind}`)
        .then((r) =>
          (Array.isArray(r) ? r : [])
            .filter((x): x is { tag: string; count?: unknown } => !!x && typeof x === "object" && typeof (x as { tag?: unknown }).tag === "string")
            .map((x) => ({ tag: x.tag, count: typeof x.count === "number" ? x.count : 0 })),
        )
        .catch(() => []),
    staleTime: 30_000,
  });

const jobActive = (j?: Job) => !!j && (j.status === "queued" || j.status === "running");

export const useJobs = (enabled = true) =>
  useQuery({
    queryKey: ["jobs"],
    enabled,
    queryFn: async () => {
      const r = await get<Job[]>(`/api/v1/jobs`);
      return Array.isArray(r) ? r.filter((j) => j && typeof j.id === "string") : [];
    },
    // Live events invalidate on every change; this only backs them up.
    refetchInterval: (q) => ((q.state.data ?? []).some(jobActive) ? 5_000 : false),
  });

export const useJob = (id?: string) =>
  useQuery({
    queryKey: ["job", id],
    queryFn: () => get<Job>(`/api/v1/jobs/${id}`),
    enabled: !!id,
    refetchInterval: (q) => (!q.state.data || jobActive(q.state.data) ? 2_000 : false),
  });

export const useJobSteps = (id?: string) =>
  useQuery({
    queryKey: ["steps", id],
    queryFn: () => get<JobStep[]>(`/api/v1/jobs/${id}/steps`),
    enabled: !!id,
    refetchInterval: (q) => (q.state.data?.some((s) => s.status === "running" || s.status === "retrying") || !q.state.data?.length ? 2_000 : false),
  });

export const useUsers = () => useQuery({ queryKey: ["users"], queryFn: () => get<User[]>("/api/v1/users"), retry: false });
export const useBindings = () =>
  useQuery({ queryKey: ["bindings"], queryFn: () => get<RoleBinding[]>("/api/v1/role-bindings") });
export const usePlugins = () =>
  useQuery({ queryKey: ["plugins"], queryFn: () => get<Plugin[]>("/api/v1/plugins"), retry: false });

export const useSecrets = () =>
  useQuery({
    queryKey: ["secrets"],
    queryFn: () => get<SecretMeta[]>(`/api/v1/secrets`),
    retry: false,
  });

export interface OnboardingInfo {
  agents_available: boolean;
  install_url: string;
  agent_url: string;
  linux: string;
}

export const useOnboardingInfo = () =>
  useQuery({
    queryKey: ["onboarding"],
    queryFn: () => get<OnboardingInfo>(`/api/v1/onboarding`),
    retry: false,
  });

const str = (v: unknown): string | null => (typeof v === "string" && v ? v : null);
const num = (v: unknown): number | null => (typeof v === "number" && Number.isFinite(v) ? v : null);

/** Console lines from any server shape: drops entries without a text line. */
export function normalizeLines(v: unknown): ConsoleLine[] {
  if (!Array.isArray(v)) return [];
  return v.flatMap((x): ConsoleLine[] => {
    if (typeof x === "string") return [{ line: x }];
    if (!x || typeof x !== "object") return [];
    const o = x as Record<string, unknown>;
    if (typeof o.line !== "string") return [];
    return [{ line: o.line, t: num(o.t) ?? undefined, step: str(o.step) ?? undefined }];
  });
}

const CONSOLE_ACTIVE = new Set(["queued", "running"]);

/**
 * Onboarding console of a `node.bootstrap` job or a manual install's token.
 * Polls every 1.5 s while the install runs. Always a complete object: an
 * older server (no endpoint) or a failed request yields `unavailable`.
 */
export function useOnboardingConsole(src: { jobId?: string | null; tokenId?: string | null }, enabled = true) {
  const q = src.jobId ? `job_id=${enc(src.jobId)}` : src.tokenId ? `token_id=${enc(src.tokenId)}` : "";
  return useQuery({
    queryKey: ["onboarding-console", q],
    enabled: enabled && !!q,
    queryFn: async (): Promise<OnboardingConsole> => {
      try {
        const r = await get<Record<string, unknown>>(`/api/v1/onboarding/console?${q}`);
        const o = r && typeof r === "object" ? r : {};
        return { status: str(o.status) ?? "unknown", error: str(o.error), server_id: str(o.server_id), lines: normalizeLines(o.lines) };
      } catch {
        return { status: "unknown", error: null, server_id: null, lines: [], unavailable: true };
      }
    },
    refetchInterval: (query) => {
      const d = query.state.data;
      if (!d) return 1_500;
      if (d.unavailable) return 15_000;
      return CONSOLE_ACTIVE.has(d.status) ? 1_500 : false;
    },
  });
}

/** Nodes being onboarded (assisted jobs and manual installs); `[]` when unavailable. */
export const useOnboardingNodes = (enabled = true) =>
  useQuery({
    queryKey: ["onboarding-nodes"],
    enabled,
    queryFn: async (): Promise<OnboardingNode[]> => {
      const r = await get<unknown>(`/api/v1/onboarding/nodes`).catch(() => []);
      if (!Array.isArray(r)) return [];
      return r.flatMap((x): OnboardingNode[] => {
        if (!x || typeof x !== "object") return [];
        const o = x as Record<string, unknown>;
        const job_id = str(o.job_id);
        const token_id = str(o.token_id);
        if (!job_id && !token_id) return [];
        return [
          {
            kind: o.kind === "manual" ? "manual" : "assisted",
            job_id,
            token_id,
            status: str(o.status) ?? "unknown",
            error: str(o.error),
            host: str(o.host),
            hostname: str(o.hostname),
            cluster_id: str(o.cluster_id),
            server_id: str(o.server_id),
            created_at: num(o.created_at),
            updated_at: num(o.updated_at),
            active: o.active === true,
          },
        ];
      });
    },
    refetchInterval: (q) => ((q.state.data ?? []).some((n) => n.active) ? 3_000 : 30_000),
  });

// ------------------------------------------------------------------ mutations

/** A mutation that invalidates the given query prefixes on success. */
export function useAction<TArgs, TResult, TCtx = unknown>(
  fn: (args: TArgs) => Promise<TResult>,
  invalidate: QueryKey[],
  /** Optimistic update: `onMutate` returns a context that `onError` uses to roll back. */
  optimistic?: { onMutate: (args: TArgs) => Promise<TCtx | undefined> | TCtx | undefined; onError: (ctx: TCtx | undefined) => void },
) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: fn,
    onMutate: optimistic?.onMutate,
    onError: (_e, _args, ctx) => optimistic?.onError(ctx),
    onSettled: async () => {
      await Promise.all(invalidate.map((k) => qc.invalidateQueries({ queryKey: k })));
    },
  });
}

const ENV_KEYS: QueryKey[] = [fleetKeys.environments, fleetKeys.projects];

/** A new environment; it lives in one cluster and one project. 409 when that cluster already has the name. */
export const useCreateEnvironment = () =>
  useAction((b: { name: string; cluster_id: string; project_id: string; protected: boolean }) => post<Environment>(`/api/v1/environments`, b), ENV_KEYS);
/** Move an environment, with its services, into another cluster. 409 on a name clash there. */
export const useMoveEnvironment = () =>
  useAction(
    ({ id, cluster_id }: { id: string; cluster_id: string }) => api<Environment>("PUT", `/api/v1/environments/${enc(id)}/cluster`, { cluster_id }),
    [...ENV_KEYS, fleetKeys.services],
  );
/** Rename an environment. 409 when its cluster already has the name. */
export const useRenameEnvironment = () =>
  useAction(({ id, name }: { id: string; name: string }) => api<Environment>("PUT", `/api/v1/environments/${enc(id)}`, { name }), ENV_KEYS);
/** Delete an empty environment. 409 while it still has services or stacks. */
export const useDeleteEnvironment = () => useAction((id: string) => api<void>("DELETE", `/api/v1/environments/${enc(id)}`), [...ENV_KEYS, fleetKeys.services]);
/** Attach an environment to another project. */
export const useSetEnvironmentProject = () =>
  useAction(
    ({ id, project_id }: { id: string; project_id: string }) => api<Environment>("PUT", `/api/v1/environments/${enc(id)}/project`, { project_id }),
    ENV_KEYS,
  );

/** Create, or rename/describe, a project. 409 on a duplicate name. */
export const useSaveProject = () =>
  useAction(
    ({ id, ...b }: { id?: string; name: string; description?: string }) => (id ? api<ProjectBase>("PUT", `/api/v1/projects/${enc(id)}`, b) : post<ProjectBase>(`/api/v1/projects`, b)),
    [fleetKeys.projects],
  );
/** Delete a project. 409 while it still has environments. */
export const useDeleteProject = () => useAction((id: string) => api<void>("DELETE", `/api/v1/projects/${enc(id)}`), ENV_KEYS);

export const useSaveCluster = () =>
  useAction(
    ({ id, ...b }: { id?: string; name: string; description: string; environment_id: string | null; abbr?: string; icon?: string }) =>
      id ? api<Cluster>("PUT", `/api/v1/clusters/${id}`, b) : post<Cluster>(`/api/v1/clusters`, b),
    [["clusters"]],
  );
export const useDeleteCluster = () =>
  useAction((id: string) => api<void>("DELETE", `/api/v1/clusters/${id}`), [["clusters"], ["servers"], fleetKeys.environments]);

export const useAssignServer = () =>
  useAction(
    ({ id, ...b }: { id: string; cluster_id: string | null; environment_id: string | null }) =>
      api<Server>("PATCH", `/api/v1/servers/${id}`, b),
    [["servers"], ["server"], ["clusters"]],
  );

export const useServerCommand = () =>
  useAction(
    ({ server, command }: { server: string; command: Record<string, string> }) =>
      post<Job>(`/api/v1/servers/${server}/commands`, { command }),
    [["jobs"]],
  );

export const useRevokeServer = () =>
  useAction(
    (b: { id: string; confirm_hostname: string; totp_code?: string }) =>
      post<void>(`/api/v1/servers/${b.id}/revoke`, { confirm_hostname: b.confirm_hostname, totp_code: b.totp_code }),
    [["servers"], ["server"]],
  );
/** Removes a node for good (revoke + delete); the name is free afterwards. */
export const useRemoveServer = () =>
  useAction(
    (b: { id: string; confirm_hostname: string; totp_code?: string }) =>
      api<void>("DELETE", `/api/v1/servers/${b.id}`, { confirm_hostname: b.confirm_hostname, totp_code: b.totp_code }),
    [["servers"], ["stacks"], ["names"]],
  );
export const useRenameServer = () =>
  useAction((b: { id: string; name: string }) => api<Server>("PUT", `/api/v1/servers/${b.id}/name`, { name: b.name }), [["servers"], ["server"]]);

/** Local names of nodes and services (D-084). */
export const useLocalNames = (enabled = true) =>
  useQuery({ queryKey: ["names"], enabled, queryFn: () => get<LocalNames>(`/api/v1/names`), refetchInterval: enabled ? 15_000 : false });

/** The suffix of every local name and where it comes from. */
export const useNameSuffix = () => useQuery({ queryKey: ["names-suffix"], queryFn: () => get<NameSuffix>(`/api/v1/names/suffix`) });

/** Sets (empty: clears the runtime setting) the suffix of every local name. Platform admins, MFA. */
export const useSetNameSuffix = () =>
  useAction((suffix: string) => api<NameSuffix>("PUT", `/api/v1/names/suffix`, { suffix }), [["names-suffix"], ["names"]]);

/** Sets (empty: clears) a node's DNS label. */
export const useSetDnsName = () =>
  useAction((b: { id: string; label: string }) => api<NodeDns>("PUT", `/api/v1/servers/${b.id}/dns-name`, { label: b.label }), [["servers"], ["server"], ["names"]]);

export interface EnrollmentToken {
  id: string;
  token: string;
  expires_at: number;
  install: { linux: string; windows: string };
}
export const useCreateEnrollmentToken = () =>
  useAction(
    (b: { ttl_minutes: number; environment_id?: string; cluster_id?: string; hostname?: string }) =>
      post<EnrollmentToken>(`/api/v1/enrollment-tokens`, b),
    [["onboarding-nodes"]],
  );

export const useProbeNode = () =>
  useMutation({
    mutationFn: (b: { host: string; port: number }) =>
      post<{ host: string; port: number; host_key: HostKey }>(`/api/v1/nodes/probe`, b),
  });

export interface OnboardRequest {
  host: string;
  port: number;
  username: string;
  host_key_fingerprint: string;
  auth: { type: "password"; password: string } | { type: "private_key"; pem: string; passphrase?: string };
  sudo_password?: string;
  cluster_id?: string;
  environment_id?: string;
  keep_credential: boolean;
  hostname?: string;
  /** Terminal account the installer creates (D-078). */
  terminal_account?: string;
  terminal_account_sudo?: boolean;
}
export const useOnboardNode = () =>
  useAction(
    (b: OnboardRequest) => post<{ job: Job }>(`/api/v1/nodes/onboard`, b),
    [["jobs"], ["secrets"], ["onboarding-nodes"]],
  );

export const useDeleteSecret = () =>
  useAction((id: string) => api<void>("DELETE", `/api/v1/secrets/${id}`), [["secrets"]]);

export const useCancelJob = () =>
  useAction((id: string) => post<void>(`/api/v1/jobs/${id}/cancel`), [["jobs"], ["job"], ["steps"], ["onboarding-nodes"], ["onboarding-console"]]);

export const useCreateUser = () =>
  useAction(
    (b: { username?: string; email?: string; display_name: string; password: string }) => post<User>("/api/v1/users", b),
    [["users"]],
  );
export const useUpdateProfile = () =>
  useAction(
    (b: { username: string; email: string; display_name: string; current_password?: string }) =>
      api<{ user: User }>("PATCH", "/api/v1/me", b),
    [["me"], ["users"]],
  );
export const useChangePassword = () =>
  useMutation({
    mutationFn: (b: { current_password: string; new_password: string }) => post<void>("/api/v1/me/password", b),
  });
export const useGrant = () =>
  useAction(
    (b: { email: string; role: string; scope: string }) => post<RoleBinding>("/api/v1/role-bindings", b),
    // ["me"] also refreshes the console capabilities when one's own roles change.
    [["bindings"], ["me"]],
  );
export const useRevokeBinding = () =>
  useAction((id: string) => api<void>("DELETE", `/api/v1/role-bindings/${id}`), [["bindings"], ["me"]]);

export const usePluginAction = () =>
  useAction(
    ({ id, action }: { id: string; action: "enable" | "disable" | "pin" | "unpin" }) =>
      post<Plugin>(`/api/v1/plugins/${enc(id)}/${action}`),
    [["plugins"]],
  );

export const useBeginTotp = () =>
  useMutation({ mutationFn: () => post<{ secret: string; otpauth_uri: string }>("/api/v1/auth/totp") });
export const useConfirmTotp = () =>
  useAction((b: { secret: string; code: string; password?: string }) => post<void>("/api/v1/auth/totp/confirm", b), [["me"]]);

// ------------------------------------------------------------------ live events

/** One WebSocket per tab; events invalidate the affected queries. */
export function useLiveEvents(enabled: boolean, onEvent?: (ev: LiveEvent) => void) {
  const qc = useQueryClient();
  const handler = useRef(onEvent);
  handler.current = onEvent;
  useEffect(() => {
    if (!enabled) return;
    let socket: WebSocket | undefined;
    let retry = 0;
    let closed = false;
    const connect = () => {
      const proto = location.protocol === "https:" ? "wss" : "ws";
      socket = new WebSocket(`${proto}://${location.host}/api/v1/events`);
      socket.onopen = () => (retry = 0);
      socket.onmessage = (m) => {
        const ev = JSON.parse(m.data as string) as LiveEvent;
        handler.current?.(ev);
        if (ev.type === "job") {
          void qc.invalidateQueries({ queryKey: ["jobs"] });
          void qc.invalidateQueries({ queryKey: ["job", ev.job_id] });
          void qc.invalidateQueries({ queryKey: ["steps", ev.job_id] });
          void qc.invalidateQueries({ queryKey: ["installations"] });
          void qc.invalidateQueries({ queryKey: fleetKeys.services });
          void qc.invalidateQueries({ queryKey: fleetKeys.serviceAny });
          if (ev.kind === "app.action") void qc.invalidateQueries({ queryKey: ["cluster-routing"] });
          if (typeof ev.kind === "string" && ev.kind.startsWith("stack.")) {
            void qc.invalidateQueries({ queryKey: ["stacks"] });
            void qc.invalidateQueries({ queryKey: ["stack"] });
          }
          if (ev.kind === "node.bootstrap") {
            void qc.invalidateQueries({ queryKey: ["onboarding-nodes"] });
            void qc.invalidateQueries({ queryKey: ["onboarding-console"] });
          }
        } else {
          void qc.invalidateQueries({ queryKey: ["servers"] });
          void qc.invalidateQueries({ queryKey: ["server", ev.server_id] });
          void qc.invalidateQueries({ queryKey: ["clusters"] });
          void qc.invalidateQueries({ queryKey: ["cluster-routing"] });
        }
      };
      socket.onclose = () => {
        if (closed) return;
        // Exponential backoff with full jitter, like the agents.
        setTimeout(connect, Math.random() * Math.min(30_000, 500 * 2 ** retry++));
      };
    };
    connect();
    return () => {
      closed = true;
      socket?.close();
    };
  }, [enabled, qc]);
}

// ------------------------------------------------------------------ SSH terminal

export interface SshEndpoint {
  server_id: string;
  host: string;
  port: number;
  host_key_fingerprint: string;
  updated_at: number;
}
export interface SshInfo {
  endpoint: SshEndpoint | null;
  ca_public_key: string;
}

export const useSshInfo = (id?: string) =>
  useQuery({ queryKey: ["ssh", id], queryFn: () => get<SshInfo>(`/api/v1/servers/${id}/ssh`), enabled: !!id, retry: false });

export const useSaveSsh = () =>
  useAction(
    ({ id, ...b }: { id: string; host: string; port: number; host_key_fingerprint: string }) =>
      api<SshEndpoint>("PUT", `/api/v1/servers/${id}/ssh`, b),
    [["ssh"]],
  );

// ------------------------------------------------------------------ App Store

export const useCatalog = () =>
  useQuery({ queryKey: ["catalog"], queryFn: () => get<Catalog>("/api/v1/catalog"), staleTime: 5 * 60_000 });

export const useInstallations = (serverId?: string) =>
  useQuery({
    queryKey: ["installations", serverId ?? "all"],
    queryFn: () => get<Installation[]>(`/api/v1/installations${serverId ? `?server_id=${enc(serverId)}` : ""}`),
    // Installs report through jobs; poll gently while anything is busy.
    refetchInterval: (q) => ((q.state.data ?? []).some((i) => i.status.endsWith("ing")) ? 3000 : false),
  });

export const useInstallPackage = () =>
  useAction(
    (b: { package: string; version: string; params: Record<string, string>; server_ids: string[] }) =>
      post<{ server_id: string; installation: Installation; job: Job }[]>("/api/v1/packages/install", b),
    [["installations"], ["jobs"]],
  );

export const usePackageAction = () =>
  useAction(
    ({ id, action }: { id: string; action: "start" | "stop" | "restart" | "status" | "uninstall" | "install" }) =>
      post<Job>(`/api/v1/installations/${id}/actions`, { action }),
    [["installations"], ["jobs"]],
  );

export const usePackageUpdates = (enabled = true) =>
  useQuery({
    queryKey: ["installations", "updates"],
    enabled,
    queryFn: () => get<UpdateRow[]>("/api/v1/package-updates"),
    refetchInterval: (q) => ((q.state.data ?? []).some((r) => r.status.endsWith("ing")) ? 3000 : 60_000),
  });

export const usePackageReleases = () =>
  useQuery({ queryKey: ["package-releases"], queryFn: () => get<PackageRelease[]>("/api/v1/package-releases"), staleTime: 60 * 60_000 });

export const useCheckUpdates = () => useAction(() => post<{ checks: number }>("/api/v1/package-updates/check", {}), [["jobs"]]);

export const useApplyUpdate = () =>
  useAction(({ id, build }: { id: string; build: string }) => post<Job>(`/api/v1/installations/${id}/update`, { build }), [["installations"], ["jobs"]]);

export const useUpdatePolicy = () =>
  useAction(
    ({ id, update_policy }: { id: string; update_policy: UpdatePolicy }) => api<Installation>("PUT", `/api/v1/installations/${id}/policy`, { update_policy }),
    [["installations"]],
  );

// ------------------------------------------------------------------ containers

export interface ContainerResult<T> {
  job_id: string;
  status: "succeeded" | "failed" | "running";
  result?: T;
  error?: string;
}
export interface ContainerSummary {
  id: string;
  name: string;
  image: string;
  state: string;
  status: string;
  created: number;
  ports: string[];
  labels: Record<string, string> | null;
}
export interface DockerInfo {
  version: string;
  containers: number;
  running: number;
  images: number;
  swarm: string | null;
  cpus: number;
  memory: number;
}

export type ContainerOp = "info" | "list" | "images" | "inspect" | "start" | "stop" | "restart" | "remove" | "logs" | "stats" | "pull";

export interface ContainerCallBody {
  op: ContainerOp;
  id?: string;
  image?: string;
  tail?: number;
  since?: string;
  /** `inspect` only: also return `env: [{key, value}]`. Needs `container.inspect_env` with MFA (the client retries after step-up). */
  reveal_env?: boolean;
}

export function containerCall<T>(server: string, body: ContainerCallBody) {
  return post<ContainerResult<T>>(`/api/v1/servers/${server}/containers`, body);
}

const isObj = (v: unknown): v is Record<string, unknown> => !!v && typeof v === "object" && !Array.isArray(v);
const txt = (v: unknown): string => (typeof v === "string" ? v : typeof v === "number" ? String(v) : "");
/** Epoch ms from Docker's RFC 3339 strings or a number; Docker's "never" (0001-01-01) parses negative and becomes `undefined`. */
const epoch = (v: unknown): number | undefined => {
  const ms = typeof v === "number" ? (v > 1e12 ? v : v * 1000) : typeof v === "string" && v ? Date.parse(v) : NaN;
  return Number.isFinite(ms) && ms > 0 ? ms : undefined;
};

/** Normalizes an `inspect` result from any agent version; never throws. */
export function normInspect(raw: unknown, fallbackId = ""): ContainerInspect {
  const v = isObj(raw) ? raw : {};
  const st = v.state;
  const stObj = isObj(st) ? st : {};
  const state = isObj(st) ? txt(stObj.Status ?? stObj.status) : txt(st);
  const ports: ContainerPort[] = [];
  if (isObj(v.ports)) {
    for (const [k, binds] of Object.entries(v.ports)) {
      const list = Array.isArray(binds) ? binds : [];
      if (list.length === 0) ports.push({ container: k, host: "" });
      for (const b of list) {
        if (!isObj(b)) continue;
        const ip = txt(b.HostIp ?? b.host_ip);
        const port = txt(b.HostPort ?? b.host_port);
        ports.push({ container: k, host: port ? `${ip || "0.0.0.0"}:${port}` : "" });
      }
    }
  } else if (Array.isArray(v.ports)) {
    for (const p of v.ports) {
      if (typeof p === "string") {
        const [host, container] = p.includes("->") ? p.split("->") : ["", p];
        ports.push({ container: (container ?? "").trim(), host: (host ?? "").trim() });
      } else if (isObj(p)) ports.push({ container: txt(p.container ?? p.target), host: txt(p.host ?? p.published) });
    }
  }
  const mounts: ContainerMount[] = Array.isArray(v.mounts)
    ? v.mounts.filter(isObj).map((m) => ({
        type: txt(m.type ?? m.Type) || undefined,
        source: txt(m.source ?? m.Source) || undefined,
        destination: txt(m.destination ?? m.Destination) || undefined,
        mode: txt(m.mode ?? m.Mode) || undefined,
        rw: typeof m.rw === "boolean" ? m.rw : typeof m.RW === "boolean" ? m.RW : undefined,
      }))
    : [];
  const networks = Array.isArray(v.networks)
    ? v.networks.map((n) => (typeof n === "string" ? n : isObj(n) ? txt(n.name) : "")).filter(Boolean)
    : isObj(v.networks)
      ? Object.keys(v.networks)
      : [];
  const labels: Record<string, string> = {};
  if (isObj(v.labels)) for (const [k, val] of Object.entries(v.labels)) labels[k] = txt(val);
  const env_keys = Array.isArray(v.env_keys) ? v.env_keys.filter((k): k is string => typeof k === "string") : [];
  const env = Array.isArray(v.env)
    ? v.env.flatMap((e): ContainerEnv[] => {
        if (isObj(e) && typeof e.key === "string") return [{ key: e.key, value: txt(e.value) }];
        if (typeof e === "string") {
          const i = e.indexOf("=");
          return [i < 0 ? { key: e, value: "" } : { key: e.slice(0, i), value: e.slice(i + 1) }];
        }
        return [];
      })
    : undefined;
  const health = isObj(stObj.Health) ? txt(stObj.Health.Status) : txt(stObj.health ?? v.health);
  const rp = v.restart_policy;
  const strs = (x: unknown): string[] | undefined =>
    Array.isArray(x) ? x.filter((a): a is string => typeof a === "string") : typeof x === "string" && x ? [x] : undefined;
  const network_ips = isObj(v.networks)
    ? Object.entries(v.networks).flatMap(([name, n]) => {
        const ip = isObj(n) ? txt(n.IPAddress ?? n.ip ?? n.ip_address) : "";
        return ip ? [{ name, ip }] : [];
      })
    : Array.isArray(v.networks)
      ? v.networks.flatMap((n) => (isObj(n) && txt(n.name) && txt(n.ip) ? [{ name: txt(n.name), ip: txt(n.ip) }] : []))
      : [];
  return {
    id: txt(v.id) || fallbackId,
    name: txt(v.name).replace(/^\//, ""),
    image: txt(v.image),
    state: state || "unknown",
    running: typeof stObj.Running === "boolean" ? stObj.Running : state === "running",
    health: health || undefined,
    exit_code: typeof stObj.ExitCode === "number" ? stObj.ExitCode : typeof stObj.exit_code === "number" ? stObj.exit_code : undefined,
    pid: typeof stObj.Pid === "number" && stObj.Pid > 0 ? stObj.Pid : undefined,
    started_at: epoch(stObj.StartedAt ?? stObj.started_at ?? v.started),
    finished_at: epoch(stObj.FinishedAt ?? stObj.finished_at),
    created_at: epoch(v.created),
    restart_policy: (isObj(rp) ? txt(rp.name ?? rp.Name) : txt(rp)) || "no",
    ports,
    mounts,
    networks,
    labels,
    env_keys,
    env,
    image_id: txt(v.image_id ?? v.ImageId) || undefined,
    entrypoint: strs(v.entrypoint ?? v.Entrypoint),
    command: strs(v.command ?? v.cmd ?? v.Cmd),
    network_ips,
  };
}

/**
 * One container's details. `reveal` asks for the environment values too; that
 * result is never cached (`gcTime: 0`). A failed request gives `error` and
 * `data: undefined`; nothing here throws in render.
 */
export const useContainerInspect = (server: string, id: string, opts: { reveal?: boolean; enabled?: boolean } = {}) =>
  useQuery({
    queryKey: ["containers", server, "inspect", id, opts.reveal ? "env" : "keys"],
    queryFn: async (): Promise<ContainerResult<ContainerInspect>> => {
      const r = await containerCall<unknown>(server, { op: "inspect", id, ...(opts.reveal ? { reveal_env: true } : {}) });
      return { ...r, result: r.result === undefined ? undefined : normInspect(r.result, id) };
    },
    enabled: opts.enabled !== false && !!server && !!id,
    retry: false,
    staleTime: opts.reveal ? 0 : 10_000,
    gcTime: opts.reveal ? 0 : undefined,
    refetchInterval: opts.reveal ? false : 15_000,
  });

/** Live CPU and memory of one container; `undefined` fields when the agent cannot tell. Never throws. */
export const useContainerStats = (server: string, id: string, enabled = true) =>
  useQuery({
    queryKey: ["containers", server, "stats", id],
    queryFn: async (): Promise<ContainerStats | null> => {
      const r = await containerCall<unknown>(server, { op: "stats", id }).catch(() => null);
      const v = r?.status === "succeeded" && isObj(r.result) ? r.result : null;
      if (!v) return null;
      const num = (x: unknown) => (typeof x === "number" && Number.isFinite(x) ? x : undefined);
      return { cpu_percent: num(v.cpu_percent), memory_bytes: num(v.memory_bytes), memory_limit: num(v.memory_limit) };
    },
    enabled: enabled && !!server && !!id,
    retry: false,
    refetchInterval: 15_000,
  });

/** Roles whose Cedar policies grant `container.inspect_env` (the server still decides). */
const ENV_REVEALERS = ["platform_admin", "workspace_admin", "operator"];
/** Whether the signed-in person may reveal container environment values; `undefined` while loading. */
export function useCanRevealEnv(): boolean | undefined {
  const me = useMe();
  if (!me.data) return me.isError ? false : undefined;
  const b = Array.isArray(me.data.bindings) ? me.data.bindings : [];
  return b.some((x) => ENV_REVEALERS.includes(x?.role));
}

/** Read-only container queries (refetched on demand). */
export const useContainerQuery = <T,>(server: string, op: "info" | "list" | "images", enabled = true) =>
  useQuery({
    queryKey: ["containers", server, op],
    queryFn: () => containerCall<T>(server, { op }),
    enabled,
    retry: false,
    staleTime: 10_000,
  });

export const useContainerAction = (server: string) =>
  useAction(
    (b: ContainerCallBody) => containerCall<unknown>(server, b),
    [["containers", server], ["jobs"]],
  );

// ------------------------------------------------------------------ services

/** Normalizes one service, so a partial row never breaks a page. */
function normService(x: Service): Service {
  return {
    ...x,
    spec: (x.spec && typeof x.spec === "object" ? x.spec : {}) as ServiceSpec,
    env: Array.isArray(x.env) ? x.env : [],
    instances: Array.isArray(x.instances) ? x.instances.filter((i) => i && typeof i.server_id === "string").map((i) => ({ ...i, status: typeof i.status === "string" ? i.status : "" })) : [],
  };
}
const isService = (x: unknown): x is Service => !!x && typeof x === "object" && typeof (x as Service).id === "string" && typeof (x as Service).name === "string";

/** Services, optionally only one environment's or one cluster's (prefer a filter: lists can be large). */
export const useServices = (environment?: string, cluster?: string | null, enabled = true) =>
  useQuery({
    queryKey: fleetKeys.serviceList(environment, cluster),
    enabled,
    queryFn: () =>
      get<unknown>(`/api/v1/services${environment ? `?environment_id=${enc(environment)}` : cluster ? `?cluster_id=${enc(cluster)}` : ""}`).then((r) =>
        (Array.isArray(r) ? r : []).filter(isService).map(normService),
      ),
    refetchInterval: (q) => ((q.state.data ?? []).some((a) => a.instances.some((i) => i.status.endsWith("ing"))) ? 3000 : false),
  });

export const useService = (id?: string) =>
  useQuery({
    queryKey: fleetKeys.service(id),
    queryFn: () => get<Service>(`/api/v1/services/${enc(id ?? "")}`).then(normService),
    enabled: !!id,
    refetchInterval: (q) => (q.state.data?.instances.some((i) => i.status.endsWith("ing")) ? 2500 : false),
  });

const serviceKeys: QueryKey[] = [fleetKeys.services, fleetKeys.serviceAny, fleetKeys.projects, ["jobs"]];

export const useSaveService = () =>
  useAction(
    ({ id, ...b }: ServiceInput & { id?: string }) => (id ? api<Service>("PUT", `/api/v1/services/${enc(id)}`, b) : post<Service>("/api/v1/services", b)),
    serviceKeys,
  );
export const useDeleteService = () => useAction((id: string) => api<void>("DELETE", `/api/v1/services/${enc(id)}`), serviceKeys);
export const usePlaceService = () =>
  useAction(
    ({ id, placements }: { id: string; placements: { server_id: string; role: string }[] }) =>
      api<ServiceInstance[]>("PUT", `/api/v1/services/${enc(id)}/instances`, placements),
    serviceKeys,
  );
export const useDeployService = () => useAction((id: string) => post<Job[]>(`/api/v1/services/${enc(id)}/deploy`), serviceKeys);
/** One action on one placement. `run` no longer exists. */
export type ServiceAction = "prepare" | "deploy" | "start" | "stop" | "restart" | "status" | "logs" | "follow" | "remove";
export const useServiceAction = () =>
  useAction(
    ({ id, server, action, tail }: { id: string; server: string; action: ServiceAction; tail?: number }) =>
      post<ContainerResult<{ state?: string; logs?: string | null }>>(`/api/v1/services/${enc(id)}/instances/${enc(server)}/actions`, { action, tail }),
    serviceKeys,
  );

// ------------------------------------------------------------------ attached storage

/** Normalizes the storage view, so a partial response never breaks the tab. */
function normAppStorage(v: Partial<ServiceStorageView> | null | undefined): ServiceStorageView {
  const text = (x: unknown) => (typeof x === "string" ? x : "");
  const list: Partial<ServiceStorageAttachment>[] = Array.isArray(v?.attachments) ? v.attachments : [];
  return {
    // Absent on a partial response: let the server decide on attach and detach.
    can_change: v?.can_change !== false,
    attachments: list
      .filter((a) => a && typeof a.access_key === "string" && a.access_key)
      .map((a) => ({
        access_key: text(a.access_key),
        store_id: text(a.store_id),
        bucket: text(a.bucket),
        access: a.access === "read" ? "read" : "readwrite",
        prefix: text(a.prefix),
        vault_item_id: text(a.vault_item_id),
        env: {
          endpoint: text(a.env?.endpoint),
          region: text(a.env?.region),
          bucket: text(a.env?.bucket),
          access_key: text(a.env?.access_key),
          secret_key: text(a.env?.secret_key),
        },
        store_name: text(a.store_name) || null,
        endpoint: text(a.endpoint) || null,
      })),
  };
}

/** Buckets attached to the service. */
export const useServiceStorage = (id?: string) =>
  useQuery({
    queryKey: fleetKeys.storage(id),
    queryFn: () => get<ServiceStorageView>(`/api/v1/services/${enc(id ?? "")}/storage`).then(normAppStorage),
    enabled: !!id,
    staleTime: 15_000,
    retry: (n, e) => !(e instanceof ApiError && e.status < 500) && n < 2,
  });

// An attachment issues a key (store keys, vault) and may create the bucket.
const appStorageKeys: QueryKey[] = [["service-storage"], ["buckets"], ["object-store-keys"], ["vault-items"]];

/** Attaches a bucket; the variables reach the nodes with the next deploy. */
export const useAttachServiceStorage = () =>
  useAction(({ id, ...b }: ServiceStorageAttach & { id: string }) => post<ServiceStorageView>(`/api/v1/services/${enc(id)}/storage`, b).then(normAppStorage), appStorageKeys);

/** Detaches a bucket: the key is revoked at once. */
export const useDetachServiceStorage = () =>
  useAction(
    ({ id, accessKey }: { id: string; accessKey: string }) => api<ServiceStorageView>("DELETE", `/api/v1/services/${enc(id)}/storage/${enc(accessKey)}`).then(normAppStorage),
    appStorageKeys,
  );

// ------------------------------------------------------------------ health checks

const asList =<T,>(v: unknown): T[] => (Array.isArray(v) ? (v as T[]) : []);

/** Normalizes a health view, so an older or partial response never breaks the tab. */
function normHealthView(v: Partial<ServiceHealthView> | null | undefined): ServiceHealthView {
  const p = (v?.policy ?? {}) as HealthPolicy;
  return {
    policy: { ...(p.default_checks === false ? { default_checks: false } : {}), startup: asList(p.startup), readiness: asList(p.readiness), liveness: asList(p.liveness) },
    summaries: asList(v?.summaries),
    legacy_healthcheck: v?.legacy_healthcheck && typeof v.legacy_healthcheck === "object" ? v.legacy_healthcheck : null,
    updated_at: typeof v?.updated_at === "number" ? v.updated_at : 0,
    // Absent on older servers: let the server decide on save.
    can_configure: v?.can_configure !== false,
    can_command: v?.can_command !== false,
    instances: asList<Partial<InstanceHealth>>(v?.instances)
      .filter((i) => i && typeof i.server_id === "string")
      .map((i) => ({
        server_id: i.server_id as string,
        status: (["healthy", "degraded", "unhealthy", "unknown"] as const).find((s) => s === i.status) ?? "unknown",
        checks: asList<InstanceCheck>(i.checks).filter((c) => c && typeof c === "object"),
      })),
    warnings: asList<HealthWarning>(v?.warnings).filter((w) => w && typeof w.message === "string"),
  };
}

/** The service's health policy (`health.view`). */
export const useServiceHealth = (id?: string, opts?: { enabled?: boolean; live?: boolean }) =>
  useQuery({
    queryKey: fleetKeys.health(id),
    queryFn: () => get<ServiceHealthView>(`/api/v1/services/${id}/health`).then(normHealthView),
    enabled: !!id && opts?.enabled !== false,
    staleTime: 15_000,
    // Per-node status changes on its own; poll while shown, stop on 4xx (older server or denied).
    refetchInterval: (q) => (opts?.live && !(q.state.error instanceof ApiError && q.state.error.status < 500) ? 15_000 : false),
    retry: (n, e) => !(e instanceof ApiError && e.status < 500) && n < 2,
  });

/**
 * Validates a draft on the server, debounced (`delay` ms after the last
 * change). Nothing is saved. `data` keeps the previous answer while the next
 * one loads; `data.key` is the draft JSON it answers.
 */
export function useValidateHealth(id: string | undefined, policy: HealthPolicy | undefined, delay = 400) {
  const json = policy ? JSON.stringify(policy) : undefined;
  const [debounced, setDebounced] = useState(json);
  useEffect(() => {
    const t = setTimeout(() => setDebounced(json), delay);
    return () => clearTimeout(t);
  }, [json, delay]);
  const q = useQuery({
    queryKey: ["service-health-validate", id, debounced],
    queryFn: () =>
      post<Partial<HealthValidation>>(`/api/v1/services/${id}/health/validate`, { policy: JSON.parse(debounced ?? "{}") }).then((r) => ({
        key: debounced ?? "",
        valid: r?.valid === true,
        errors: asList<HealthValidation["errors"][number]>(r?.errors),
        summaries: asList<HealthValidation["summaries"][number]>(r?.summaries),
        runs_code: r?.runs_code === true,
      })),
    enabled: !!id && debounced !== undefined,
    staleTime: Infinity,
    retry: false,
    placeholderData: (prev) => prev,
  });
  return { ...q, pending: json !== debounced || q.isFetching, current: q.data?.key === json };
}

/** Replaces the policy; 409 when the app changed since `expected_updated_at`. */
export const useSaveHealth = () => {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, policy, expected_updated_at }: { id: string; policy: HealthPolicy; expected_updated_at: number }) =>
      api<ServiceHealthView>("PUT", `/api/v1/services/${id}/health`, { policy, expected_updated_at }).then(normHealthView),
    onSuccess: (v, { id }) => {
      qc.setQueryData(fleetKeys.health(id), v);
      void qc.invalidateQueries({ queryKey: fleetKeys.service(id) });
    },
  });
};

// ------------------------------------------------------------------ telemetry

export interface MetricSample {
  t: number;
  cpu: number;
  mem_used: number;
  mem_total: number;
  swap_used: number;
  swap_total: number;
  disk_used: number;
  disk_total: number;
  rx: number;
  tx: number;
  load1: number;
  uptime: number;
  cpus: number;
}

/**
 * Live node metrics. Polls incrementally (`since` the newest sample held) at
 * `intervalMs`; polling also tells the server someone is watching, which makes
 * the agent report every couple of seconds. `intervalMs = 0` pauses.
 */
export interface Volume {
  mount_point: string;
  device: string;
  file_system: string;
  total: number;
  available: number;
  used: number;
  removable: boolean;
}

export function useNodeMetrics(server: string, intervalMs: number, keepMs: number) {
  const [samples, setSamples] = useState<MetricSample[]>([]);
  const [volumes, setVolumes] = useState<Volume[]>([]);
  const [error, setError] = useState<unknown>();
  const last = useRef(0);
  useEffect(() => {
    last.current = 0;
    setSamples([]);
  }, [server]);
  useEffect(() => {
    let stop = false;
    const tick = async () => {
      try {
        const r = await get<{ samples: MetricSample[]; volumes: Volume[] }>(`/api/v1/servers/${enc(server)}/metrics?since=${last.current}`);
        if (stop) return;
        setError(undefined);
        setVolumes(r.volumes ?? []);
        if (r.samples.length) {
          last.current = r.samples[r.samples.length - 1]?.t ?? last.current;
          const cutoff = Date.now() - keepMs;
          setSamples((s) => [...s, ...r.samples].filter((x) => x.t >= cutoff));
        }
      } catch (e) {
        if (!stop) setError(e);
      }
    };
    void tick();
    if (!intervalMs) return () => void (stop = true);
    const id = setInterval(() => void tick(), intervalMs);
    return () => {
      stop = true;
      clearInterval(id);
    };
  }, [server, intervalMs, keepMs]);
  return { samples, volumes, error };
}

export interface ProbeResult {
  server_id?: string;
  hostname?: string;
  role?: string;
  target: string;
  method?: "icmp" | "tcp";
  reachable: boolean;
  sent?: number;
  received?: number;
  loss_percent?: number;
  latency_ms?: number | null;
  error?: string;
}
export interface ProbeRun {
  t: number;
  results: ProbeResult[];
}
export interface Peer {
  server_id: string;
  hostname: string;
  role: string;
  status: string;
  target: string;
}

export const useConnectivity = (server: string) =>
  useQuery({
    queryKey: ["connectivity", server],
    queryFn: () => get<{ peers: Peer[]; runs: ProbeRun[] }>(`/api/v1/servers/${enc(server)}/connectivity`),
  });

export function useRunProbe(server: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (count: number) =>
      post<{ status: string; error?: string; run?: ProbeRun }>(`/api/v1/servers/${enc(server)}/connectivity`, { count }),
    onSettled: () => void qc.invalidateQueries({ queryKey: ["connectivity", server] }),
  });
}

// ------------------------------------------------------------ Forge self-update

/** One Forge release from the update feed. */
export interface ForgeRelease {
  version: string;
  notes: string;
  url: string;
  published_at: string | null;
  /** The running version (only in the releases list). */
  current?: boolean;
}

export type ForgeUpdateState = "idle" | "downloading" | "verifying" | "installing" | "restarting" | "failed";

/** `GET /system/update`, normalized: every field has a value, even on an older server or an error. */
export interface ForgeUpdateStatus {
  current: string;
  latest: ForgeRelease | null;
  available: boolean;
  checked_at: number | null;
  state: ForgeUpdateState;
  message: string | null;
  enabled: boolean;
  can_update: boolean;
  target: string;
  feed_url: string | null;
  /** HTTP status when the request failed (403: no permission, 404: older server). */
  error_status: number | null;
  error: string | null;
}

export const FORGE_UPDATE_KEY = ["system-update"] as const;
export const FORGE_RELEASES_KEY = ["system-update-releases"] as const;
export const FORGE_BUSY_STATES: ForgeUpdateState[] = ["downloading", "verifying", "installing", "restarting"];
const FORGE_STATES: ForgeUpdateState[] = ["idle", ...FORGE_BUSY_STATES, "failed"];

type Raw = Record<string, unknown>;
const asRec = (v: unknown): Raw => (v && typeof v === "object" && !Array.isArray(v) ? (v as Raw) : {});
const asStr = (v: unknown): string => (typeof v === "string" ? v : "");
const asOptStr = (v: unknown): string | null => (typeof v === "string" && v ? v : null);
const asNum = (v: unknown): number | null => (typeof v === "number" && Number.isFinite(v) ? v : null);

function normRelease(raw: unknown): ForgeRelease | null {
  const r = asRec(raw);
  const version = asStr(r.version);
  if (!version) return null;
  return { version, notes: asStr(r.notes), url: asStr(r.url), published_at: asOptStr(r.published_at), current: r.current === true };
}

export function normForgeUpdate(raw: unknown, err?: unknown): ForgeUpdateStatus {
  const r = asRec(raw);
  const state = FORGE_STATES.find((s) => s === r.state) ?? "idle";
  return {
    current: asStr(r.current),
    latest: normRelease(r.latest),
    available: r.available === true,
    checked_at: asNum(r.checked_at),
    state,
    message: asOptStr(r.message),
    enabled: r.enabled === true,
    can_update: r.can_update === true,
    target: asStr(r.target),
    feed_url: asOptStr(r.feed_url),
    error_status: err instanceof ApiError ? err.status : err ? 0 : null,
    error: err ? (err instanceof Error ? err.message : String(err)) : null,
  };
}

/** Forge self-update status; never throws (errors are folded into the result). */
export const useForgeUpdate = (enabled = true) =>
  useQuery({
    queryKey: FORGE_UPDATE_KEY,
    enabled,
    queryFn: () => get<unknown>("/api/v1/system/update").then((r) => normForgeUpdate(r), (e: unknown) => normForgeUpdate(null, e)),
    refetchInterval: (q) => (FORGE_BUSY_STATES.includes(q.state.data?.state ?? "idle") ? 1500 : 10 * 60_000),
    staleTime: 30_000,
    retry: false,
  });

export interface ForgeReleases {
  current: string;
  /** Newest first; never older than the running version. */
  releases: ForgeRelease[];
  /** False when the server has no releases endpoint (older server). */
  supported: boolean;
  error: string | null;
}

/** Releases newer than or equal to the running version, newest first. Never throws. */
export const useForgeReleases = (enabled = true) =>
  useQuery({
    queryKey: FORGE_RELEASES_KEY,
    enabled,
    staleTime: 60_000,
    retry: false,
    queryFn: (): Promise<ForgeReleases> =>
      get<unknown>("/api/v1/system/update/releases").then(
        (raw) => {
          const r = asRec(raw);
          const releases = (Array.isArray(r.releases) ? r.releases : []).map(normRelease).filter((x): x is ForgeRelease => !!x);
          return { current: asStr(r.current), releases, supported: true, error: null };
        },
        (e: unknown) => ({
          current: "",
          releases: [],
          supported: !(e instanceof ApiError && (e.status === 404 || e.status === 405)),
          error: e instanceof ApiError && (e.status === 404 || e.status === 405) ? null : e instanceof Error ? e.message : String(e),
        }),
      ),
  });

export const useCheckForgeUpdate = () => {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => post<unknown>("/api/v1/system/update/check", {}).then((r) => normForgeUpdate(r)),
    onSuccess: (r) => {
      qc.setQueryData(FORGE_UPDATE_KEY, r);
      void qc.invalidateQueries({ queryKey: FORGE_RELEASES_KEY });
    },
  });
};

export interface ApplyForgeUpdate {
  version: string;
  agents: "rollout" | "none";
  batch?: number;
}

export const useApplyForgeUpdate = () => {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (b: ApplyForgeUpdate) => post<unknown>("/api/v1/system/update/apply", b.agents === "rollout" ? b : { version: b.version, agents: "none" }),
    onSuccess: () => void qc.invalidateQueries({ queryKey: FORGE_UPDATE_KEY }),
  });
};

// ------------------------------------------------------------------ tunnels (D-100)
//
// Every reader folds a missing endpoint (older server) or a failed request
// into `supported: false` / `error` and empty lists; the pages never crash.

const missing = (e: unknown) => e instanceof ApiError && (e.status === 404 || e.status === 405);
const errMsg = (e: unknown) => (e instanceof Error ? e.message : String(e));

function normTarget(v: unknown, fallback: Raw): TunnelTarget {
  const r = asRec(v);
  const kind = asStr(r.kind) || asStr(fallback.target_kind) || (fallback.port !== undefined ? "port" : "");
  const ref = asStr(r.ref) || asStr(fallback.target_ref);
  return { kind, ref, label: asStr(r.label) || ref || "—" };
}

export function normTunnel(raw: unknown): Tunnel | null {
  const r = asRec(raw);
  const id = asStr(r.id);
  if (!id) return null;
  return {
    id,
    user: asStr(r.user) || asStr(r.user_name),
    user_id: asStr(r.user_id),
    server_id: asStr(r.server_id),
    server: asStr(r.server) || asStr(r.server_name),
    target: normTarget(r.target, r),
    port: asNum(r.port),
    role: asOptStr(r.role),
    bind: asStr(r.bind),
    opened_at: asNum(r.opened_at) ?? asNum(r.started_at),
    expires_at: asNum(r.expires_at),
    closed_at: asNum(r.closed_at),
    close_reason: asOptStr(r.close_reason),
    bytes_up: asNum(r.bytes_up) ?? 0,
    bytes_down: asNum(r.bytes_down) ?? 0,
    connections: asNum(r.connections) ?? 0,
  };
}

export interface TunnelList {
  tunnels: Tunnel[];
  /** False when this server has no such endpoint. */
  supported: boolean;
  error: string | null;
}

// Unknown `/api` paths on older servers answer with the SPA's HTML (200), which
// `get` turns into `{}`: an answer without the expected list means "no endpoint".
const hasList = (raw: unknown, ...keys: string[]) => Array.isArray(raw) || keys.some((k) => Array.isArray(asRec(raw)[k]));

const tunnelList = (raw: unknown): TunnelList => {
  const r = asRec(raw);
  const list = Array.isArray(raw) ? raw : Array.isArray(r.tunnels) ? r.tunnels : Array.isArray(r.history) ? r.history : [];
  return { tunnels: list.map(normTunnel).filter((t): t is Tunnel => !!t), supported: hasList(raw, "tunnels", "history"), error: null };
};

const emptyTunnels = (e: unknown): TunnelList => ({ tunnels: [], supported: !missing(e), error: missing(e) ? null : errMsg(e) });

/** Open tunnels: the caller's own, or everyone's with `all`. Never throws. */
export const useTunnels = (all = false) =>
  useQuery({
    queryKey: ["tunnels", all ? "all" : "mine"],
    queryFn: (): Promise<TunnelList> =>
      get<unknown>(`/api/v1/tunnels${all ? "?all=true" : ""}`).then(tunnelList, emptyTunnels),
    refetchInterval: 10_000,
    staleTime: 5_000,
    retry: false,
  });

/** Closed tunnels with their reason (`GET /tunnels?state=closed`; `all=true` = everyone's, for admins). */
export const useTunnelHistory = (enabled = true) =>
  useQuery({
    queryKey: ["tunnels", "history"],
    enabled,
    queryFn: (): Promise<TunnelList> => get<unknown>("/api/v1/tunnels?all=true&state=closed").then(tunnelList, emptyTunnels),
    refetchInterval: 30_000,
    staleTime: 10_000,
    retry: false,
  });

export const useCloseTunnel = () => useAction((id: string) => api<void>("DELETE", `/api/v1/tunnels/${enc(id)}`), [["tunnels"]]);

export function normGrant(raw: unknown): TunnelGrant | null {
  const r = asRec(raw);
  const id = asStr(r.id);
  if (!id) return null;
  const ports = Array.isArray(r.ports) ? r.ports.map(String).join(", ") : asStr(r.ports);
  return {
    id,
    user: asStr(r.user) || asStr(r.user_name),
    user_id: asStr(r.user_id),
    target: normTarget(r.target, r),
    server: asOptStr(r.server) ?? asOptStr(r.server_name),
    server_id: asOptStr(r.server_id),
    ports: ports || "*",
    expires_at: asNum(r.expires_at),
    revoked_at: asNum(r.revoked_at),
    created_by: asStr(r.created_by),
    created_at: asNum(r.created_at),
    note: asOptStr(r.note),
  };
}

export interface TunnelGrants {
  grants: TunnelGrant[];
  /** `[tunnels] allow_never_expiry` on the server. */
  allow_never_expiry: boolean;
  /** The caller may issue never-expiring grants (setting on and an admin role). */
  can_never_expire: boolean;
  supported: boolean;
  error: string | null;
}

/** Explicit tunnel grants the caller may see. Never throws. */
export const useTunnelGrants = (enabled = true) =>
  useQuery({
    queryKey: ["tunnel-grants"],
    enabled,
    queryFn: (): Promise<TunnelGrants> =>
      get<unknown>("/api/v1/tunnel-grants").then(
        (raw) => {
          const r = asRec(raw);
          const list = Array.isArray(raw) ? raw : Array.isArray(r.grants) ? r.grants : [];
          return {
            grants: list.map(normGrant).filter((g): g is TunnelGrant => !!g),
            allow_never_expiry: r.allow_never_expiry === true,
            can_never_expire: r.can_never_expire === true,
            supported: hasList(raw, "grants"),
            error: null,
          };
        },
        (e: unknown) => ({ grants: [], allow_never_expiry: false, can_never_expire: false, supported: !missing(e), error: missing(e) ? null : errMsg(e) }),
      ),
    refetchInterval: 30_000,
    staleTime: 10_000,
    retry: false,
  });

export interface TunnelGrantInput {
  user_id: string;
  target: { kind: string; ref: string; server_id?: string; ports?: string };
  /** Either a lifetime in minutes or `never_expires` (needs `allow_never_expiry`). */
  lifetime_minutes?: number;
  never_expires?: boolean;
  note?: string;
}

export const useCreateTunnelGrant = () => useAction((b: TunnelGrantInput) => post<unknown>("/api/v1/tunnel-grants", b), [["tunnel-grants"]]);
export const useRevokeTunnelGrant = () => useAction((id: string) => api<void>("DELETE", `/api/v1/tunnel-grants/${enc(id)}`), [["tunnel-grants"], ["tunnels"]]);

export function normServicePort(raw: unknown): ServicePort | null {
  const r = asRec(raw);
  const port = asNum(r.port);
  const server_id = asStr(r.server_id);
  if (port === null || !server_id) return null;
  return {
    kind: asStr(r.kind),
    environment: asOptStr(r.environment),
    name: asStr(r.name),
    label: asStr(r.label) || asStr(r.name),
    server_id,
    server: asStr(r.server) || asStr(r.server_name),
    port,
    role: asOptStr(r.role),
    dial_ip: asOptStr(r.dial_ip),
  };
}

export interface ServicePorts {
  entries: ServicePort[];
  supported: boolean;
  error: string | null;
}

/** Ports a service listens on, per node (`GET /api/v1/service-ports?service=…&kind=…`). Never throws. */
export const useServicePorts = (service: string | undefined, kind: "app" | "stack" | "package", enabled = true) =>
  useQuery({
    queryKey: ["service-ports", kind, service ?? ""],
    enabled: enabled && !!service,
    queryFn: (): Promise<ServicePorts> =>
      get<unknown>(`/api/v1/service-ports?service=${enc(service ?? "")}&kind=${kind}`).then(
        (raw) => {
          const r = asRec(raw);
          const list = Array.isArray(raw) ? raw : Array.isArray(r.entries) ? r.entries : Array.isArray(r.ports) ? r.ports : [];
          return { entries: list.map(normServicePort).filter((p): p is ServicePort => !!p), supported: hasList(raw, "entries", "ports"), error: null };
        },
        (e: unknown) => ({ entries: [], supported: !missing(e), error: missing(e) ? null : errMsg(e) }),
      ),
    staleTime: 15_000,
    refetchInterval: 30_000,
    retry: false,
  });

/** Accounts a terminal can log in as on a node: saved for the node, and reported by it. */
export const useServerLogins = (server: string | undefined) =>
  useQuery({
    queryKey: ["server-logins", server],
    queryFn: () => get<{ logins: string[]; saved?: string[]; discovered?: string[] }>(`/api/v1/servers/${server}/logins`),
    enabled: !!server,
  });
