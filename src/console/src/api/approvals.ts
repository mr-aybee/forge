// Approvals (MCP Ask mode, issue #6): a change an AI connector asked for
// waits here until its owner approves or denies it in the console.
//
// Every reader normalizes the server's JSON into complete objects: an older
// server without the endpoint (404), a failed request or a missing field
// yields a flagged result or a default, never a crash. The request shown is
// data from an AI agent; it is displayed, never interpreted.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { ApiError, get, post } from "./client";

const enc = encodeURIComponent;

export type ApprovalStatus = "pending" | "approved" | "executed" | "failed" | "denied" | "expired";

export interface ApprovalRequest {
  method: string;
  uri: string;
  body: unknown;
  arguments: unknown;
}

export interface Approval {
  id: string;
  kind: string;
  /** Unknown values from newer servers are kept as strings. */
  status: ApprovalStatus | string;
  /** Name of the connector (AI agent) that asked. */
  requester: string;
  connection_id: string | null;
  /** Tool name, e.g. `forge_refresh_stack`. */
  action: string;
  request: ApprovalRequest;
  summary: string;
  mfa_required: boolean;
  created_at: number | null;
  expires_at: number | null;
  decided_by: string | null;
  decided_at: number | null;
  result: unknown;
  error: string | null;
  user_id: string;
}

/** `ok`, endpoint missing (`off`, older server), not allowed (`forbidden`), or another error. */
export type Availability = "ok" | "off" | "forbidden" | "error";

type Obj = Record<string, unknown>;
const obj = (v: unknown): Obj => (v && typeof v === "object" && !Array.isArray(v) ? (v as Obj) : {});
const s = (v: unknown) => (typeof v === "string" ? v : "");
const opt = (v: unknown) => (typeof v === "string" && v ? v : null);
const ms = (v: unknown) => (typeof v === "number" && Number.isFinite(v) && v > 0 ? v : null);

function availability(e: unknown): Availability {
  if (e instanceof ApiError && (e.status === 404 || e.status === 405)) return "off";
  if (e instanceof ApiError && (e.status === 403 || e.status === 401)) return "forbidden";
  return "error";
}

export function toApproval(v: unknown): Approval | null {
  const o = obj(v);
  const id = s(o.id);
  if (!id) return null;
  const r = obj(o.request);
  return {
    id,
    kind: s(o.kind),
    status: s(o.status) || "pending",
    requester: s(o.requester) || "Unnamed connector",
    connection_id: opt(o.connection_id),
    action: s(o.action),
    request: { method: s(r.method).toUpperCase() || "POST", uri: s(r.uri), body: r.body ?? null, arguments: r.arguments ?? null },
    summary: s(o.summary),
    mfa_required: o.mfa_required === true,
    created_at: ms(o.created_at),
    expires_at: ms(o.expires_at),
    decided_by: opt(o.decided_by),
    decided_at: ms(o.decided_at),
    result: o.result ?? null,
    error: opt(o.error),
    user_id: s(o.user_id),
  };
}

/** The status as of `now`: a pending request past its expiry is expired, even before the next poll. */
export const effectiveStatus = (a: Approval, now: number): string =>
  a.status === "pending" && a.expires_at !== null && a.expires_at <= now ? "expired" : a.status;

/** A readable title for a tool name: `forge_refresh_stack` → "Refresh stack". */
export function actionTitle(action: string): string {
  const words = action.replace(/^forge_/, "").replace(/[_-]+/g, " ").trim();
  return words ? words.charAt(0).toUpperCase() + words.slice(1) : "Unknown action";
}

export const APPROVALS_KEY = ["approvals"] as const;

export interface ApprovalsResult {
  status: Availability;
  error?: string;
  approvals: Approval[];
}

async function list(query: string): Promise<ApprovalsResult> {
  try {
    const items = obj(await get<unknown>(`/api/v1/approvals${query}`)).approvals;
    return {
      status: "ok",
      approvals: (Array.isArray(items) ? items : []).map(toApproval).filter((a): a is Approval => a !== null),
    };
  } catch (e) {
    return { status: availability(e), error: e instanceof Error ? e.message : String(e), approvals: [] };
  }
}

/** Your approvals, newest first; polled every 10 s while the page is open. */
export const useApprovals = () =>
  useQuery({
    queryKey: [...APPROVALS_KEY, "mine"],
    retry: false,
    refetchInterval: 10_000,
    staleTime: 5_000,
    queryFn: () => list(""),
  });

/** Count of your pending approvals for the sidebar badge; 0 when unavailable. */
export const usePendingApprovalCount = () =>
  useQuery({
    queryKey: [...APPROVALS_KEY, "pending-count"],
    retry: false,
    refetchInterval: 30_000,
    staleTime: 15_000,
    queryFn: async () => {
      const r = await list("?status=pending");
      const now = Date.now();
      return r.approvals.filter((a) => effectiveStatus(a, now) === "pending").length;
    },
  });

export interface ApprovalResult {
  /** `missing`: not found or not yours. */
  status: "ok" | "missing" | "forbidden" | "error";
  error?: string;
  approval: Approval | null;
}

/** One approval (opened from the agent's link); polled while it is pending. */
export const useApproval = (id: string | undefined) =>
  useQuery({
    queryKey: [...APPROVALS_KEY, "one", id],
    enabled: !!id,
    retry: false,
    staleTime: 5_000,
    refetchInterval: (q) => (q.state.data?.approval?.status === "pending" ? 10_000 : false),
    queryFn: async (): Promise<ApprovalResult> => {
      try {
        const a = toApproval(await get<unknown>(`/api/v1/approvals/${enc(id ?? "")}`));
        return a ? { status: "ok", approval: a } : { status: "error", error: "The server sent an unreadable approval.", approval: null };
      } catch (e) {
        const status = e instanceof ApiError && e.status === 404 ? "missing" : e instanceof ApiError && e.status === 403 ? "forbidden" : "error";
        return { status, error: e instanceof Error ? e.message : String(e), approval: null };
      }
    },
  });

/** Approve (runs the request once, as you) or deny; resolves to the approval afterwards. */
export function useDecideApproval() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async ({ id, approve }: { id: string; approve: boolean }): Promise<Approval | null> =>
      toApproval(await post<unknown>(`/api/v1/approvals/${enc(id)}/${approve ? "approve" : "deny"}`)),
    onSuccess: (a) => {
      if (a) qc.setQueryData<ApprovalResult>([...APPROVALS_KEY, "one", a.id], { status: "ok", approval: a });
    },
    onSettled: () => void qc.invalidateQueries({ queryKey: APPROVALS_KEY }),
  });
}
