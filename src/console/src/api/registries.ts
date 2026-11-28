// Container registry credentials (D-157). Server:
// `src/forge/crates/apps/forge-server/src/api/registries.rs`. The secret is write-only: the
// list never returns it. Mutations need `registry.manage` and an MFA-verified
// sign-in (the client handles step-up).
import { useQuery } from "@tanstack/react-query";

import { api, get, post } from "./client";
import { useAction } from "./hooks";

export type RegistryKind = "docker_hub" | "ghcr" | "gitlab" | "ecr" | "custom";
export const REGISTRY_KINDS: RegistryKind[] = ["docker_hub", "ghcr", "gitlab", "ecr", "custom"];

export interface Registry {
  id: string;
  workspace_id: string;
  name: string;
  kind: RegistryKind;
  host: string;
  username: string;
  created_by: string;
  created_at?: number;
  updated_at?: number;
}

/** Body of create and update. On update, omit `secret` to keep the stored one. */
export interface RegistryBody {
  name: string;
  kind: RegistryKind;
  host: string;
  username: string;
  secret?: string;
}

export const registryKeys = {
  all: ["registries"] as const,
};

type Obj = Record<string, unknown>;
const str = (v: unknown) => (typeof v === "string" ? v : "");
const num = (v: unknown) => (typeof v === "number" && Number.isFinite(v) ? v : undefined);

function normRegistry(v: unknown): Registry {
  const o: Obj = v && typeof v === "object" ? (v as Obj) : {};
  const kind = str(o.kind) as RegistryKind;
  return {
    id: str(o.id),
    workspace_id: str(o.workspace_id),
    name: str(o.name),
    kind: REGISTRY_KINDS.includes(kind) ? kind : "custom",
    host: str(o.host),
    username: str(o.username),
    created_by: str(o.created_by),
    created_at: num(o.created_at),
    updated_at: num(o.updated_at),
  };
}

const enc = encodeURIComponent;

export const useRegistries = () =>
  useQuery({
    queryKey: registryKeys.all,
    queryFn: () => get<unknown>("/api/v1/registries").then((v) => (Array.isArray(v) ? v.map(normRegistry) : [])),
    staleTime: 30_000,
  });

/** 409 when the host is already added; 422 when a Docker Hub entry is not `docker.io`. */
export const useCreateRegistry = () => useAction((b: RegistryBody) => post<unknown>("/api/v1/registries", b), [registryKeys.all]);
export const useUpdateRegistry = () =>
  useAction(({ id, body }: { id: string; body: RegistryBody }) => api<unknown>("PUT", `/api/v1/registries/${enc(id)}`, body), [registryKeys.all]);
export const useDeleteRegistry = () => useAction((id: string) => api<void>("DELETE", `/api/v1/registries/${enc(id)}`), [registryKeys.all]);
