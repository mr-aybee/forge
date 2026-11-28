import { useQuery, useQueryClient } from "@tanstack/react-query";

import { ApiError, api, get } from "./client";
import { useAction } from "./hooks";

const enc = encodeURIComponent;

export type PinKind = "project" | "service" | "node" | "cluster";
export const PIN_KINDS: PinKind[] = ["project", "service", "node", "cluster"];

export interface Pin {
  kind: PinKind;
  id: string;
  label: string;
  sublabel?: string;
  position: number;
  created_at: number;
}

/** `available: false` when the server has no pins endpoint (older server): the UI hides pinning. */
export interface PinList {
  available: boolean;
  items: Pin[];
}

/** The most pins the server keeps per user. */
export const PIN_LIMIT = 50;

export const pinKeys = {
  all: ["pins"] as const,
};

const isKind = (k: unknown): k is PinKind => typeof k === "string" && (PIN_KINDS as string[]).includes(k);

function toPins(r: unknown): Pin[] {
  const items = r && typeof r === "object" ? (r as { items?: unknown }).items : undefined;
  if (!Array.isArray(items)) return [];
  return items
    .flatMap((x): Pin[] => {
      if (!x || typeof x !== "object") return [];
      const o = x as Record<string, unknown>;
      if (!isKind(o.kind) || typeof o.id !== "string") return [];
      return [
        {
          kind: o.kind,
          id: o.id,
          label: typeof o.label === "string" && o.label ? o.label : o.id,
          sublabel: typeof o.sublabel === "string" && o.sublabel ? o.sublabel : undefined,
          position: typeof o.position === "number" ? o.position : 0,
          created_at: typeof o.created_at === "number" ? o.created_at : 0,
        },
      ];
    })
    .sort((a, b) => a.position - b.position);
}

/** Your pins in order. Never throws: a missing endpoint or a failure gives `{ available: false, items: [] }`. */
export const usePins = (enabled = true) =>
  useQuery({
    queryKey: pinKeys.all,
    enabled,
    retry: false,
    staleTime: 30_000,
    queryFn: (): Promise<PinList> =>
      get<unknown>("/api/v1/me/pins").then(
        (r) => ({ available: true, items: toPins(r) }),
        // 404: older server; anything else: the sidebar stays quiet and hides the section.
        () => ({ available: false, items: [] }),
      ),
  });

/** Whether `kind/id` is pinned, and whether pinning is available at all. */
export function useIsPinned(kind: PinKind, id: string | undefined): { available: boolean; pinned: boolean; full: boolean } {
  const d = usePins().data;
  const items = d?.items ?? [];
  return { available: d?.available === true && !!id, pinned: !!id && items.some((p) => p.kind === kind && p.id === id), full: items.length >= PIN_LIMIT };
}

export interface PinRef {
  kind: PinKind;
  id: string;
  /** Shown optimistically until the server answers. */
  label?: string;
  sublabel?: string;
}

/** Optimistic edits of the cached list; rolled back on error. */
function useOptimistic(edit: (items: Pin[], args: PinRef) => Pin[]) {
  const qc = useQueryClient();
  return {
    onMutate: async (args: PinRef) => {
      await qc.cancelQueries({ queryKey: pinKeys.all });
      const prev = qc.getQueryData<PinList>(pinKeys.all);
      if (prev) qc.setQueryData<PinList>(pinKeys.all, { ...prev, items: edit(prev.items, args) });
      return prev;
    },
    onError: (prev: PinList | undefined) => {
      if (prev) qc.setQueryData(pinKeys.all, prev);
    },
  };
}

/** Pins an item (idempotent). 404 unknown item; 409/422 past the limit. */
export const usePin = () =>
  useAction(
    (p: PinRef) => api<unknown>("PUT", `/api/v1/me/pins/${enc(p.kind)}/${enc(p.id)}`),
    [pinKeys.all],
    useOptimistic((items, p) =>
      items.some((x) => x.kind === p.kind && x.id === p.id)
        ? items
        : [...items, { kind: p.kind, id: p.id, label: p.label ?? p.id, sublabel: p.sublabel, position: (items.at(-1)?.position ?? 0) + 1, created_at: Date.now() }],
    ),
  );

/** Unpins an item; a pin already gone counts as done. */
export const useUnpin = () =>
  useAction(
    (p: PinRef) =>
      api<void>("DELETE", `/api/v1/me/pins/${enc(p.kind)}/${enc(p.id)}`).catch((e: unknown) => {
        if (e instanceof ApiError && e.status === 404) return;
        throw e;
      }),
    [pinKeys.all],
    useOptimistic((items, p) => items.filter((x) => !(x.kind === p.kind && x.id === p.id))),
  );

/** Saves a new order (the full list). */
export function useReorderPins() {
  const qc = useQueryClient();
  return useAction(
    (items: Pin[]) => api<unknown>("PUT", "/api/v1/me/pins/order", { items: items.map((p) => ({ kind: p.kind, id: p.id })) }),
    [pinKeys.all],
    {
      onMutate: async (items: Pin[]) => {
        await qc.cancelQueries({ queryKey: pinKeys.all });
        const prev = qc.getQueryData<PinList>(pinKeys.all);
        if (prev) qc.setQueryData<PinList>(pinKeys.all, { ...prev, items: items.map((p, i) => ({ ...p, position: i })) });
        return prev;
      },
      onError: (prev: PinList | undefined) => {
        if (prev) qc.setQueryData(pinKeys.all, prev);
      },
    },
  );
}

/** The page a pin opens, as router `to`/`search`. */
export function pinTarget(p: { kind: PinKind; id: string }): { to: string; search?: { sel: string } } {
  switch (p.kind) {
    case "project":
      return { to: "/projects", search: { sel: `project:${p.id}` } };
    case "service":
      return { to: `/services/${enc(p.id)}` };
    case "node":
      return { to: `/nodes/${enc(p.id)}` };
    case "cluster":
      return { to: `/clusters/${enc(p.id)}` };
  }
}
