// A node's Docker Engine API endpoint (mutual TLS on dockerd): state, enable,
// disable and check. Server: `src/apps/forge-server/src/api/docker.rs`.
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef } from "react";

import { api, get, post } from "./client";
import { useAction, useJob } from "./hooks";
import type { Job } from "./types";

const enc = encodeURIComponent;
const base = (server: string) => `/api/v1/servers/${enc(server)}/docker/endpoint`;

export type DockerEndpointState = "disabled" | "pending" | "enabled" | "failed" | "unknown";

/** A node's Docker API endpoint; every field but `state` may be missing. */
export interface DockerEndpoint {
  state: DockerEndpointState;
  host?: string;
  port?: number;
  engine_version?: string;
  api_version?: string;
  /** SHA-256 of the node's Docker CA certificate (hex). */
  ca_fingerprint?: string;
  /** When dockerd's certificate expires (epoch ms). */
  daemon_cert_not_after?: number;
  last_error?: string;
  updated_at?: number;
}

export interface DockerEnableBody {
  /** Omitted = the address the agent connects from. */
  host?: string;
  port?: number;
  listen?: string;
  acknowledge_restart: boolean;
}

export interface JobStarted {
  job_id: string;
}

export const dockerKeys = {
  all: ["docker-endpoint"] as const,
  endpoint: (server: string) => ["docker-endpoint", server] as const,
};

const STATES: DockerEndpointState[] = ["disabled", "pending", "enabled", "failed"];
const str = (v: unknown) => (typeof v === "string" && v ? v : undefined);
const num = (v: unknown) => (typeof v === "number" && Number.isFinite(v) ? v : undefined);

/** Normalizes any server shape so the panel never trips over a missing field. */
export function normDockerEndpoint(v: unknown): DockerEndpoint {
  const o = (v && typeof v === "object" ? v : {}) as Record<string, unknown>;
  const s = str(o.state) as DockerEndpointState | undefined;
  return {
    state: s && STATES.includes(s) ? s : "unknown",
    host: str(o.host),
    port: num(o.port),
    engine_version: str(o.engine_version),
    api_version: str(o.api_version),
    ca_fingerprint: str(o.ca_fingerprint),
    daemon_cert_not_after: num(o.daemon_cert_not_after),
    last_error: str(o.last_error),
    updated_at: num(o.updated_at),
  };
}

/** The endpoint's state; polls while it is `pending`. Errors (incl. 404 on older servers) surface as `error`. */
export const useDockerEndpoint = (server: string, enabled = true) =>
  useQuery({
    queryKey: dockerKeys.endpoint(server),
    queryFn: () => get<unknown>(base(server)).then(normDockerEndpoint),
    enabled: enabled && !!server,
    retry: false,
    staleTime: 10_000,
    refetchInterval: (q) => (q.state.data?.state === "pending" ? 3_000 : false),
  });

/** Starts a `docker.endpoint.enable` job (MFA step-up handled by the client). */
export const useEnableDockerEndpoint = (server: string) =>
  useAction((b: DockerEnableBody) => post<JobStarted>(base(server), b), [dockerKeys.endpoint(server), ["jobs"]]);

/** Starts a `docker.endpoint.disable` job; `force` forgets it without asking the node. */
export const useDisableDockerEndpoint = (server: string) =>
  useAction(
    (force: boolean) => api<JobStarted>("DELETE", `${base(server)}${force ? "?force=true" : ""}`),
    [dockerKeys.endpoint(server), ["jobs"]],
  );

/** Asks the daemon now; the fresh view replaces the cached one. */
export function useCheckDockerEndpoint(server: string) {
  const qc = useQueryClient();
  return useAction(
    () =>
      post<unknown>(`${base(server)}/check`, {}).then((v) => {
        const ep = normDockerEndpoint(v);
        qc.setQueryData(dockerKeys.endpoint(server), ep);
        return ep;
      }),
    [],
  );
}

/**
 * Follows an endpoint job; when it ends, refreshes the endpoint and calls
 * `onDone` once with the finished job.
 */
export function useDockerEndpointJob(server: string, jobId: string | undefined, onDone: (j: Job) => void) {
  const job = useJob(jobId);
  const qc = useQueryClient();
  const status = job.data?.status;
  const done = status === "succeeded" || status === "failed" || status === "cancelled";
  // Latest callback without re-firing the effect when its identity changes.
  const cb = useRef(onDone);
  cb.current = onDone;
  const finished = done ? job.data : undefined;
  useEffect(() => {
    if (!finished) return;
    void qc.invalidateQueries({ queryKey: dockerKeys.endpoint(server) });
    cb.current(finished);
  }, [finished?.id, server, qc]); // once per finished job
  return job;
}
