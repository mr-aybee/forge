// Object storage: stores (managed RustFS or connected S3), buckets and objects.
import { useMutation, useQuery, useQueryClient, type QueryKey } from "@tanstack/react-query";

import { ApiError, api, csrfToken, get, post } from "./client";
import type { Bucket, BucketGrant, BucketGrantCreate, Job, MyBucketGrant, ObjectPage, ObjectStore, StoreAdopt, StoreConnect, StoreInstall, StoreKey, StoreKeyCreate, StoreKeyIssued, StoreMember, StoreTest } from "./types";

const enc = encodeURIComponent;
const base = (id: string) => `/api/v1/object-stores/${enc(id)}`;

/** Normalizes a store so pages never trip over a missing field (older servers, partial rows). */
function normStore(s: Partial<ObjectStore> | null | undefined): ObjectStore {
  const o = s ?? {};
  return {
    ...o,
    id: o.id ?? "",
    environment_id: typeof o.environment_id === "string" && o.environment_id ? o.environment_id : null,
    limited: o.limited === true,
    by_grant: o.by_grant === true,
    name: o.name ?? o.slug ?? "unnamed",
    provider: o.provider ?? "s3",
    mode: o.mode === "managed" ? "managed" : "external",
    status: o.status ?? "unknown",
    members: Array.isArray(o.members)
      ? o.members.map(
          (m): StoreMember => ({
            ...m,
            server_id: m?.server_id ?? "",
            data_paths: Array.isArray(m?.data_paths) ? m.data_paths.filter((p) => typeof p === "string") : [],
          }),
        )
      : [],
  };
}

/** Statuses while a lifecycle job runs. Not `endsWith("ing")`: "running" is settled. */
export const BUSY = new Set(["installing", "removing", "starting", "stopping", "restarting", "checking", "adopting", "disabling", "enabling"]);
export const storeBusy = (s?: { status?: string | null }) => BUSY.has(s?.status ?? "");

/** Members that count towards health: everything except removed ones. */
export const liveMembers = (s: ObjectStore) => s.members.filter((m) => m.status !== "removed");
export const runningMembers = (s: ObjectStore) => s.members.filter((m) => m.status === "running");

/** Data path of the App Store `rustfs` package; a member using it is the adopted container. */
export const ADOPTED_DATA = "/var/lib/forge/pkgs/rustfs";
/** Adopted from the App Store: Forge runs the existing `forge-rustfs` container and cannot reinstall it. */
export const isAdopted = (s: ObjectStore) => s.mode === "managed" && s.members.some((m) => m.data_paths.length === 1 && m.data_paths[0] === ADOPTED_DATA);
/** Access keys are issued on managed RustFS stores only (through its admin API). */
export const keysSupported = (s: ObjectStore) => s.mode === "managed" && s.provider === "rustfs";

export const useObjectStores = () =>
  useQuery({
    queryKey: ["object-stores"],
    queryFn: () => get<ObjectStore[]>("/api/v1/object-stores").then((l) => (Array.isArray(l) ? l.map(normStore) : [])),
    retry: (n, e) => !(e instanceof ApiError && (e.status === 404 || e.status === 403)) && n < 2,
    refetchInterval: (q) => ((q.state.data ?? []).some(storeBusy) ? 3000 : 30_000),
    staleTime: 5_000,
  });

export const useObjectStore = (id?: string) =>
  useQuery({
    queryKey: ["object-store", id],
    queryFn: () => get<ObjectStore>(base(id ?? "")).then(normStore),
    enabled: !!id,
    retry: (n, e) => !(e instanceof ApiError && e.status < 500) && n < 2,
    refetchInterval: (q) => (storeBusy(q.state.data) ? 2500 : 30_000),
  });

const storeKeys: QueryKey[] = [["object-stores"], ["object-store"], ["jobs"]];

function useAction<A, R>(fn: (a: A) => Promise<R>, keys: QueryKey[]) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: fn,
    onSuccess: async () => {
      await Promise.all(keys.map((k) => qc.invalidateQueries({ queryKey: k })));
    },
  });
}

export const useInstallStore = () =>
  useAction((b: StoreInstall) => post<{ store?: ObjectStore; job?: Job; jobs?: Job[] }>("/api/v1/object-stores", b), storeKeys);

export const useAdoptStore = () => useAction((b: StoreAdopt) => post<{ store?: ObjectStore; job?: Job }>("/api/v1/object-stores/adopt", b), [...storeKeys, ["installations"]]);

export const useConnectStore = () => useAction((b: StoreConnect) => post<ObjectStore>("/api/v1/object-stores/connect", b), storeKeys);

/** `409` code: buckets of the store are attached to services of other environments; repeat with `force`. */
export const ATTACHED_ELSEWHERE = "attached_elsewhere";

/** Assigns a store to an environment, or makes it shared again with `null`. `force` assigns it although other environments' services use its buckets. */
export const useSetStoreOwner = () =>
  useAction(
    ({ id, environment_id, force }: { id: string; environment_id: string | null; force?: boolean }) =>
      api<ObjectStore>("PUT", `${base(id)}/owner`, { environment_id, force: force || undefined }).then(normStore),
    storeKeys,
  );

export type StoreAction = "install" | "start" | "stop" | "restart" | "status" | "uninstall" | "disconnect" | "disable" | "enable";

export const useStoreAction = () =>
  useAction(
    ({ id, action, delete_data, confirm }: { id: string; action: StoreAction; delete_data?: boolean; confirm?: string }) =>
      post<{ jobs?: Job[]; disconnected?: string; skipped?: { server_id: string; result: string }[] }>(`${base(id)}/actions`, { action, delete_data, confirm }),
    storeKeys,
  );

export const useTestStore = () => useMutation({ mutationFn: (id: string) => post<StoreTest>(`${base(id)}/test`, {}) });

export const useBuckets = (id?: string) =>
  useQuery({
    queryKey: ["buckets", id],
    queryFn: () => get<Bucket[]>(`${base(id ?? "")}/buckets`).then((l) => (Array.isArray(l) ? l : [])),
    enabled: !!id,
    retry: false,
    staleTime: 10_000,
  });

export const useCreateBucket = () =>
  useAction(({ id, name }: { id: string; name: string }) => post<{ name: string }>(`${base(id)}/buckets`, { name }), [["buckets"]]);

export const useDeleteBucket = () =>
  useAction(({ id, bucket }: { id: string; bucket: string }) => api<void>("DELETE", `${base(id)}/buckets/${enc(bucket)}`), [["buckets"]]);

export const objectsKey = (id: string, bucket: string, prefix: string) => ["objects", id, bucket, prefix];

/** One page of a bucket listing under `prefix` (delimiter `/`). */
export const fetchObjects = (id: string, bucket: string, prefix: string, token?: string | null) =>
  get<ObjectPage>(`${base(id)}/buckets/${enc(bucket)}/objects?prefix=${enc(prefix)}${token ? `&token=${enc(token)}` : ""}&limit=200`).then(
    (p): ObjectPage => ({
      objects: Array.isArray(p?.objects) ? p.objects : [],
      prefixes: Array.isArray(p?.prefixes) ? p.prefixes : [],
      next_token: p?.next_token ?? null,
    }),
  );

export const objectUrl = (id: string, bucket: string, key: string) => `${base(id)}/buckets/${enc(bucket)}/object?key=${enc(key)}`;

export const useDeleteObject = () =>
  useAction(({ id, bucket, key }: { id: string; bucket: string; key: string }) => api<void>("DELETE", objectUrl(id, bucket, key)), [["objects"]]);

export const MAX_UPLOAD = 5 * 1024 ** 3;

/**
 * Uploads a file as the raw request body. Uses XMLHttpRequest because fetch
 * has no upload progress. Same-origin cookie + CSRF header like `api()`.
 */
export function uploadObject(id: string, bucket: string, key: string, file: File, onProgress: (sent: number, total: number) => void, signal?: AbortSignal) {
  return new Promise<void>((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    xhr.open("PUT", objectUrl(id, bucket, key));
    xhr.withCredentials = true;
    xhr.setRequestHeader("Accept", "application/json");
    xhr.setRequestHeader("Content-Type", file.type || "application/octet-stream");
    const csrf = csrfToken();
    if (csrf) xhr.setRequestHeader("X-CSRF-Token", csrf);
    xhr.upload.onprogress = (e) => onProgress(e.loaded, e.lengthComputable ? e.total : file.size);
    xhr.onload = () => {
      if (xhr.status >= 200 && xhr.status < 300) return resolve();
      let title = "error";
      let detail = xhr.statusText || `HTTP ${xhr.status}`;
      try {
        const d = JSON.parse(xhr.responseText) as { title?: string; detail?: string };
        title = d.title ?? title;
        detail = d.detail ?? detail;
      } catch {
        /* not JSON */
      }
      reject(new ApiError(xhr.status, title, detail));
    };
    xhr.onerror = () => reject(new ApiError(0, "network", "Upload failed: network error"));
    xhr.onabort = () => reject(new ApiError(0, "aborted", "Upload cancelled"));
    signal?.addEventListener("abort", () => xhr.abort());
    xhr.send(file);
  });
}

// ------------------------------------------------------------------ access keys

const str = (v: unknown) => (typeof v === "string" && v ? v : null);

function normKey(k: Partial<StoreKey> | null | undefined): StoreKey {
  const o = k ?? {};
  return {
    access_key: str(o.access_key) ?? "",
    name: str(o.name),
    description: str(o.description),
    status: str(o.status),
    expiration: str(o.expiration),
    vault_item_id: str(o.vault_item_id),
    bucket: str(o.bucket),
    access: str(o.access),
    managed_by_forge: o.managed_by_forge === true,
  };
}

/** Access keys of a managed RustFS store. 409 = not supported (connected store), 404 = older server. */
export const useStoreKeys = (id?: string, enabled = true) =>
  useQuery({
    queryKey: ["object-store-keys", id],
    queryFn: () => get<StoreKey[]>(`${base(id ?? "")}/keys`).then((l) => (Array.isArray(l) ? l.map(normKey).filter((k) => k.access_key) : [])),
    enabled: !!id && enabled,
    retry: (n, e) => !(e instanceof ApiError && e.status < 500) && n < 1,
    staleTime: 10_000,
  });

// Keys live in the vault too, so its list changes with them.
const keyKeys: QueryKey[] = [["object-store-keys"], ["vault-items"]];

export const useCreateKey = () => useAction(({ id, ...b }: StoreKeyCreate & { id: string }) => post<StoreKeyIssued>(`${base(id)}/keys`, b), keyKeys);

export const useRotateKey = () =>
  useAction(({ id, accessKey }: { id: string; accessKey: string }) => post<StoreKeyIssued>(`${base(id)}/keys/${enc(accessKey)}/rotate`, {}), keyKeys);

export const useRevokeKey = () => useAction(({ id, accessKey }: { id: string; accessKey: string }) => api<void>("DELETE", `${base(id)}/keys/${enc(accessKey)}`), keyKeys);

// ------------------------------------------------------------------ bucket grants

const grantKeys = {
  all: ["bucket-grants"] as const,
  store: (id: string | undefined, all: boolean) => ["bucket-grants", "store", id, all] as const,
  mine: ["bucket-grants", "mine"] as const,
};

function normGrant(g: Partial<BucketGrant> | null | undefined): BucketGrant {
  const o = g ?? {};
  return {
    id: o.id ?? "",
    store_id: o.store_id ?? "",
    store: o.store ?? "",
    user_id: o.user_id ?? "",
    user: o.user ?? o.user_id ?? "",
    bucket: o.bucket ?? "",
    access: o.access === "readwrite" ? "readwrite" : "read",
    created_by: o.created_by ?? "",
    created_by_name: str(o.created_by_name),
    created_at: o.created_at ?? 0,
    expires_at: typeof o.expires_at === "number" ? o.expires_at : null,
    never_expires: o.never_expires === true,
    revoked_at: typeof o.revoked_at === "number" ? o.revoked_at : null,
    note: o.note ?? "",
    status: o.status === "expired" || o.status === "revoked" ? o.status : "active",
  };
}

const grantList = (r: { grants?: Partial<BucketGrant>[] } | null | undefined) => (Array.isArray(r?.grants) ? r.grants.map(normGrant).filter((g) => g.id) : []);

/**
 * The bucket grants on a store: live ones, or with `all` the expired and
 * revoked ones too. Needs `storage.manage` with an MFA-verified sign-in, so
 * the caller decides when to ask (`enabled`).
 */
export const useStoreGrants = (id: string | undefined, all: boolean, enabled: boolean) =>
  useQuery({
    queryKey: grantKeys.store(id, all),
    queryFn: () => get<{ grants?: BucketGrant[] }>(`${base(id ?? "")}/grants${all ? "?all=true" : ""}`).then(grantList),
    enabled: !!id && enabled,
    retry: (n, e) => !(e instanceof ApiError && e.status < 500) && n < 1,
    staleTime: 10_000,
  });

const myGrant = ({ id, store_id, store, bucket, access, expires_at, status }: BucketGrant): MyBucketGrant => ({ id, store_id, store, bucket, access, expires_at, status });

/** The signed-in person's own live grants: the buckets they may open, and how. */
export const useMyBucketGrants = (enabled = true) =>
  useQuery({
    queryKey: grantKeys.mine,
    queryFn: () => get<{ grants?: Partial<BucketGrant>[] }>("/api/v1/object-stores/grants/mine").then((r) => grantList(r).map(myGrant)),
    enabled,
    retry: (n, e) => !(e instanceof ApiError && e.status < 500) && n < 1,
    staleTime: 10_000,
  });

export const useCreateBucketGrant = () => useAction(({ id, ...b }: BucketGrantCreate & { id: string }) => post<BucketGrant>(`${base(id)}/grants`, b).then(normGrant), [[...grantKeys.all]]);

export const useRevokeBucketGrant = () => useAction(({ id, grantId }: { id: string; grantId: string }) => api<void>("DELETE", `${base(id)}/grants/${enc(grantId)}`), [[...grantKeys.all]]);
