// Releases and deployments of a service.
import { useQuery } from "@tanstack/react-query";

import { ApiError, get, post } from "./client";
import { useAction } from "./hooks";

const enc = encodeURIComponent;

// ------------------------------------------------------------------ types

export type ReleaseArtifact =
  | { kind: "image"; image: string; digest?: string | null }
  | { kind: "git"; repo: string; commit: string; path?: string }
  | { kind: "archive"; url: string; sha256?: string; strip_components?: number };

export interface RolloutStep {
  weight: number;
  pause?: number | null;
  approval?: boolean;
  analysis?: unknown;
}

export interface RolloutPolicy {
  strategy: string;
  replacement?: string;
  batch_size?: number;
  steps?: RolloutStep[];
  approval?: boolean;
  approval_timeout_secs?: number;
  allow_downtime?: boolean;
  startup_timeout_secs?: number;
  stabilization_secs?: number;
  drain_secs?: number;
  auto_rollback?: boolean;
  retain_releases?: number;
}

export interface Release {
  id: string;
  service_id: string;
  number: number;
  artifact: ReleaseArtifact | null;
  policy: RolloutPolicy | null;
  source_sha256: string | null;
  description: string;
  created_by: string;
  created_at: number;
  /** Only on the single-release endpoint: the YAML it was applied from. */
  source?: string | null;
}

export type DeploymentState = "pending" | "running" | "paused" | "awaiting_approval" | "succeeded" | "failed" | "aborted" | "rolled_back";

export interface Deployment {
  id: string;
  service_id: string;
  release_id: string;
  release_number: number;
  from_release: number | null;
  kind: string;
  strategy: string;
  state: DeploymentState;
  request: string | null;
  job_id: string | null;
  step: number;
  error: string | null;
  created_by: string;
  created_at: number;
  updated_at: number;
  finished_at: number | null;
}

export interface DeploymentEvent {
  deployment_id: string;
  seq: number;
  at: number;
  level: "info" | "warn" | "error";
  server_id: string | null;
  message: string;
}

export interface DeploymentDetail extends Deployment {
  service: string;
  steps: string[];
  events: DeploymentEvent[];
  can_control: boolean;
  can_approve: boolean;
}

export interface ReleaseSlot {
  service_id: string;
  server_id: string;
  slot: string;
  release_number: number;
  weight: number;
  state: string;
  ports: Record<string, number>;
  updated_at: number;
}

export interface SlotsView {
  current_release: number;
  slots: ReleaseSlot[];
}

export interface PlanView {
  service: string;
  environment: string;
  creates_service: boolean;
  release: number;
  artifact: string;
  strategy: string;
  slotted: boolean;
  downtime: boolean;
  nodes: string[];
  added_nodes: string[];
  removed_nodes: string[];
  steps: string[];
  notes: string[];
}

export interface DeployStarted {
  deployment: Deployment;
}

export interface DeployYaml {
  yaml: string;
  environment?: string;
}

// ------------------------------------------------------------------ normalizers

const arr = <T>(v: unknown): T[] => (Array.isArray(v) ? (v as T[]) : []);
const str = (v: unknown) => (typeof v === "string" ? v : "");
const num = (v: unknown) => (typeof v === "number" && Number.isFinite(v) ? v : 0);

const ACTIVE = new Set<string>(["pending", "running", "paused", "awaiting_approval"]);
export const deploymentActive = (d?: { state?: string } | null) => ACTIVE.has(d?.state ?? "");

function normDeployment(d: Partial<Deployment> | null | undefined): Deployment {
  const o = d ?? {};
  return {
    id: str(o.id),
    service_id: str(o.service_id),
    release_id: str(o.release_id),
    release_number: num(o.release_number),
    from_release: typeof o.from_release === "number" ? o.from_release : null,
    kind: str(o.kind) || "apply",
    strategy: str(o.strategy),
    state: (str(o.state) || "pending") as DeploymentState,
    request: o.request ?? null,
    job_id: o.job_id ?? null,
    step: num(o.step),
    error: o.error ?? null,
    created_by: str(o.created_by),
    created_at: num(o.created_at),
    updated_at: num(o.updated_at),
    finished_at: typeof o.finished_at === "number" ? o.finished_at : null,
  };
}

function normEvents(v: unknown): DeploymentEvent[] {
  return arr<Partial<DeploymentEvent>>(v).map((e) => ({
    deployment_id: str(e?.deployment_id),
    seq: num(e?.seq),
    at: num(e?.at),
    level: e?.level === "warn" || e?.level === "error" ? e.level : "info",
    server_id: e?.server_id ?? null,
    message: str(e?.message),
  }));
}

function normRelease(r: Partial<Release> | null | undefined): Release {
  const o = r ?? {};
  return {
    id: str(o.id),
    service_id: str(o.service_id),
    number: num(o.number),
    artifact: o.artifact && typeof o.artifact === "object" ? o.artifact : null,
    policy: o.policy && typeof o.policy === "object" ? o.policy : null,
    source_sha256: o.source_sha256 ?? null,
    description: str(o.description),
    created_by: str(o.created_by),
    created_at: num(o.created_at),
    source: typeof o.source === "string" ? o.source : null,
  };
}

/** One-line artifact label: image@digest, repo#sha, archive url. */
export function artifactLabel(a: ReleaseArtifact | null | undefined): string {
  if (!a) return "—";
  if (a.kind === "image") return a.digest ? `${a.image}@${a.digest.replace(/^sha256:/, "").slice(0, 12)}` : a.image;
  if (a.kind === "git") return `${a.repo}#${(a.commit ?? "").slice(0, 8)}${a.path ? ` (${a.path})` : ""}`;
  if (a.kind === "archive") return a.url;
  return "—";
}

// ------------------------------------------------------------------ queries

export const deployKeys = {
  all: ["deploy"] as const,
  releases: (appId: string) => [...deployKeys.all, "releases", appId] as const,
  release: (appId: string, n: number) => [...deployKeys.all, "release", appId, n] as const,
  deployments: (appId: string) => [...deployKeys.all, "deployments", appId] as const,
  slots: (appId: string) => [...deployKeys.all, "slots", appId] as const,
  deployment: (id: string) => [...deployKeys.all, "deployment", id] as const,
};

const noRetry4xx = (n: number, e: unknown) => !(e instanceof ApiError && e.status < 500) && n < 2;

export const useReleases = (appId: string) =>
  useQuery({
    queryKey: deployKeys.releases(appId),
    queryFn: () => get<Release[]>(`/api/v1/services/${enc(appId)}/releases?limit=50`).then((l) => arr<Release>(l).map(normRelease)),
    retry: noRetry4xx,
    staleTime: 5_000,
  });

export const useRelease = (appId: string, n?: number) =>
  useQuery({
    queryKey: deployKeys.release(appId, n ?? 0),
    queryFn: () => get<Release>(`/api/v1/services/${enc(appId)}/releases/${n}`).then(normRelease),
    enabled: n !== undefined,
    retry: noRetry4xx,
  });

export const useAppDeployments = (appId: string) =>
  useQuery({
    queryKey: deployKeys.deployments(appId),
    queryFn: () => get<Deployment[]>(`/api/v1/services/${enc(appId)}/deployments?limit=30`).then((l) => arr<Deployment>(l).map(normDeployment)),
    retry: noRetry4xx,
    refetchInterval: (q) => ((q.state.data ?? []).some(deploymentActive) ? 3000 : 30_000),
    staleTime: 2_000,
  });

export const useSlots = (appId: string) =>
  useQuery({
    queryKey: deployKeys.slots(appId),
    queryFn: () =>
      get<SlotsView>(`/api/v1/services/${enc(appId)}/slots`).then(
        (v): SlotsView => ({
          current_release: num(v?.current_release),
          slots: arr<Partial<ReleaseSlot>>(v?.slots).map((s) => ({
            service_id: str(s?.service_id),
            server_id: str(s?.server_id),
            slot: str(s?.slot),
            release_number: num(s?.release_number),
            weight: num(s?.weight),
            state: str(s?.state) || "unknown",
            ports: s?.ports && typeof s.ports === "object" ? s.ports : {},
            updated_at: num(s?.updated_at),
          })),
        }),
      ),
    retry: noRetry4xx,
    refetchInterval: 10_000,
    staleTime: 2_000,
  });

/** One deployment; polls every 2 s while it is active, stops when it ends. */
export const useDeployment = (id?: string) =>
  useQuery({
    queryKey: deployKeys.deployment(id ?? ""),
    queryFn: () =>
      get<DeploymentDetail>(`/api/v1/deployments/${enc(id ?? "")}`).then(
        (d): DeploymentDetail => ({
          ...normDeployment(d),
          service: str(d?.service),
          steps: arr<unknown>(d?.steps).filter((s): s is string => typeof s === "string"),
          events: normEvents(d?.events),
          can_control: d?.can_control === true,
          can_approve: d?.can_approve === true,
        }),
      ),
    enabled: !!id,
    retry: noRetry4xx,
    refetchInterval: (q) => (deploymentActive(q.state.data) ? 2000 : false),
  });

// ------------------------------------------------------------------ mutations

export type DeploymentControl = "pause" | "resume" | "abort" | "approve" | "reject";

export const useControlDeployment = () =>
  useAction(({ id, action }: { id: string; action: DeploymentControl }) => post<Deployment>(`/api/v1/deployments/${enc(id)}/${action}`, {}).then(normDeployment), [deployKeys.all]);

const started = (r: Partial<DeployStarted> | null | undefined): DeployStarted => ({ deployment: normDeployment(r?.deployment) });

export const useRollback = () =>
  useAction(({ appId, release }: { appId: string; release?: number }) => post<DeployStarted>(`/api/v1/services/${enc(appId)}/rollback`, release === undefined ? {} : { release }).then(started), [deployKeys.all, ["services"]]);

export const useRedeployRelease = () =>
  useAction(({ appId, release }: { appId: string; release: number }) => post<DeployStarted>(`/api/v1/services/${enc(appId)}/deployments`, { release }).then(started), [deployKeys.all, ["services"]]);

export const usePlanYaml = () =>
  useAction(
    (body: DeployYaml) =>
      post<PlanView>("/api/v1/deployments/plan", body).then(
        (p): PlanView => ({
          service: str(p?.service),
          environment: str(p?.environment),
          creates_service: p?.creates_service === true,
          release: num(p?.release),
          artifact: typeof p?.artifact === "string" ? p.artifact : artifactLabel(p?.artifact as ReleaseArtifact | undefined),
          strategy: str(p?.strategy),
          slotted: p?.slotted === true,
          downtime: p?.downtime === true,
          nodes: arr<string>(p?.nodes),
          added_nodes: arr<string>(p?.added_nodes),
          removed_nodes: arr<string>(p?.removed_nodes),
          steps: arr<string>(p?.steps),
          notes: arr<string>(p?.notes),
        }),
      ),
    [],
  );

export const useApplyYaml = () => useAction((body: DeployYaml) => post<DeployStarted>("/api/v1/deployments", body).then(started), [deployKeys.all, ["services"]]);
