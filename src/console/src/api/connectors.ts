// MCP connectors for AI agents (Claude, ChatGPT, Cursor; issue #6): the
// consent step of a connector's OAuth sign-in, Connected apps and the
// registered connectors.
//
// Every reader normalizes the server's JSON into complete objects: a Forge
// with connectors turned off (404), a caller without `user.manage` (403), an
// older server or a failed request yields a flagged result, never a crash.
// No token passes through this module; the consent answer only returns the
// connector's redirect URI (with a one-time code) for the browser to follow.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { ApiError, get, post } from "./client";

const enc = encodeURIComponent;

// ------------------------------------------------------------------ types

/** What a connected agent may do. Unknown values from newer servers are kept as strings. */
export type AgentMode = "read_only" | "ask" | "auto";
export const MODE_LABEL: Record<string, string> = { read_only: "Read-only", ask: "Ask", auto: "Auto" };
export const modeLabel = (m: string | null | undefined) => (m ? (MODE_LABEL[m] ?? m) : "—");

export interface Connection {
  id: string;
  user_id: string;
  client_id: string;
  client_name: string;
  redirect_host: string;
  mode: string;
  created_at: number | null;
  last_used_at: number | null;
  revoked_at: number | null;
}

export interface ConnectorClient {
  client_id: string;
  name: string;
  redirect_uris: string[];
  created_at: number | null;
  revoked_at: number | null;
  last_used_at: number | null;
}

export interface ConsentRequest {
  request_id: string;
  client: { client_id: string; name: string; registered_at: number | null };
  redirect_uri: string;
  redirect_host: string;
  resource: string;
  /** Modes this Forge allows, in order; may be empty. */
  modes: AgentMode[];
  default_mode: AgentMode | null;
  /** Login of the signed-in person. */
  user: string;
}

/** `ok`, connectors turned off or missing (`off`), not allowed (`forbidden`), or another error. */
export type Availability = "ok" | "off" | "forbidden" | "error";

// ------------------------------------------------------------------ normalizing

type Obj = Record<string, unknown>;
const obj = (v: unknown): Obj => (v && typeof v === "object" && !Array.isArray(v) ? (v as Obj) : {});
const s = (v: unknown) => (typeof v === "string" ? v : "");
const ms = (v: unknown) => (typeof v === "number" && Number.isFinite(v) && v > 0 ? v : null);
const MODES: AgentMode[] = ["read_only", "ask", "auto"];
const asMode = (v: unknown): AgentMode | null => MODES.find((m) => m === v) ?? null;

function availability(e: unknown): Availability {
  if (e instanceof ApiError && (e.status === 404 || e.status === 405)) return "off";
  if (e instanceof ApiError && (e.status === 403 || e.status === 401)) return "forbidden";
  return "error";
}

function toConnection(v: unknown): Connection | null {
  const o = obj(v);
  const id = s(o.id);
  if (!id) return null;
  return {
    id,
    user_id: s(o.user_id),
    client_id: s(o.client_id),
    client_name: s(o.client_name) || "Unnamed connector",
    redirect_host: s(o.redirect_host),
    mode: s(o.mode),
    created_at: ms(o.created_at),
    last_used_at: ms(o.last_used_at),
    revoked_at: ms(o.revoked_at),
  };
}

function toClient(v: unknown): ConnectorClient | null {
  const o = obj(v);
  const client_id = s(o.client_id);
  if (!client_id) return null;
  return {
    client_id,
    name: s(o.name) || "Unnamed connector",
    redirect_uris: Array.isArray(o.redirect_uris) ? o.redirect_uris.filter((u): u is string => typeof u === "string") : [],
    created_at: ms(o.created_at),
    revoked_at: ms(o.revoked_at),
    last_used_at: ms(o.last_used_at),
  };
}

/** Host of a URI for display; the URI itself when it does not parse. */
export function uriHost(uri: string): string {
  try {
    return new URL(uri).host || uri;
  } catch {
    return uri;
  }
}

// ------------------------------------------------------------------ Connected apps

export interface ConnectionsResult {
  status: Availability;
  error?: string;
  connections: Connection[];
}

export const CONNECTED_APPS_KEY = ["connected-apps"] as const;

/** Your connections, or everyone's with `all` (`user.manage`; `forbidden` otherwise). */
export const useConnectedApps = (all: boolean, enabled = true) =>
  useQuery({
    queryKey: [...CONNECTED_APPS_KEY, all ? "all" : "mine"],
    enabled,
    retry: false,
    queryFn: async (): Promise<ConnectionsResult> => {
      try {
        const r = await get<unknown>(`/api/v1/connected-apps${all ? "?all=true" : ""}`);
        const list = obj(r).connections;
        return {
          status: "ok",
          connections: (Array.isArray(list) ? list : []).map(toConnection).filter((c): c is Connection => c !== null),
        };
      } catch (e) {
        return { status: availability(e), error: e instanceof Error ? e.message : String(e), connections: [] };
      }
    },
  });

export interface ClientsResult {
  status: Availability;
  error?: string;
  clients: ConnectorClient[];
}

export const OAUTH_CLIENTS_KEY = ["oauth-clients"] as const;

/** Registered connectors (`user.manage`). `forbidden` doubles as "not an administrator". */
export const useOAuthClients = () =>
  useQuery({
    queryKey: OAUTH_CLIENTS_KEY,
    retry: false,
    queryFn: async (): Promise<ClientsResult> => {
      try {
        const r = await get<unknown>("/api/v1/oauth/clients");
        const list = obj(r).clients;
        return {
          status: "ok",
          clients: (Array.isArray(list) ? list : []).map(toClient).filter((c): c is ConnectorClient => c !== null),
        };
      } catch (e) {
        return { status: availability(e), error: e instanceof Error ? e.message : String(e), clients: [] };
      }
    },
  });

export function useRevokeConnection() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => post<unknown>(`/api/v1/connected-apps/${enc(id)}/revoke`),
    onSettled: () => void qc.invalidateQueries({ queryKey: CONNECTED_APPS_KEY }),
  });
}

export function useRevokeClient() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (clientId: string) => post<unknown>(`/api/v1/oauth/clients/${enc(clientId)}/revoke`),
    onSettled: () => {
      void qc.invalidateQueries({ queryKey: OAUTH_CLIENTS_KEY });
      void qc.invalidateQueries({ queryKey: CONNECTED_APPS_KEY });
    },
  });
}

// ------------------------------------------------------------------ consent

export interface ConsentResult {
  /** `missing`: expired, already answered, or started by someone else. */
  status: "ok" | "missing" | "forbidden" | "error";
  error?: string;
  request: ConsentRequest | null;
}

/** The pending sign-in request a connector started. Never refetched: it is answered once. */
export const useConsentRequest = (id: string) =>
  useQuery({
    queryKey: ["oauth-consent", id],
    enabled: !!id,
    retry: false,
    staleTime: Infinity,
    gcTime: 0,
    refetchInterval: false,
    refetchOnWindowFocus: false,
    queryFn: async (): Promise<ConsentResult> => {
      try {
        const o = obj(await get<unknown>(`/api/v1/oauth/consent/${enc(id)}`));
        const c = obj(o.client);
        // Always Read-only, Ask, Auto (least to most power), whatever order the server sends.
        const modes = MODES.filter((m) => Array.isArray(o.modes) && o.modes.includes(m));
        const redirect_uri = s(o.redirect_uri);
        return {
          status: "ok",
          request: {
            request_id: s(o.request_id) || id,
            client: { client_id: s(c.client_id), name: s(c.name) || "Unnamed connector", registered_at: ms(c.registered_at) },
            redirect_uri,
            redirect_host: s(o.redirect_host) || uriHost(redirect_uri),
            resource: s(o.resource),
            modes,
            default_mode: asMode(o.default_mode),
            user: s(o.user),
          },
        };
      } catch (e) {
        const status = e instanceof ApiError && e.status === 404 ? "missing" : e instanceof ApiError && e.status === 403 ? "forbidden" : "error";
        return { status, error: e instanceof Error ? e.message : String(e), request: null };
      }
    },
  });

/** Approve (with a mode) or deny; resolves to where the browser goes next. */
export function useAnswerConsent(id: string) {
  return useMutation({
    mutationFn: async (d: { approve: true; mode: AgentMode } | { approve: false }): Promise<string> => {
      const r = obj(await post<unknown>(`/api/v1/oauth/consent/${enc(id)}`, d));
      const redirect = s(r.redirect);
      // An absolute URI: https, loopback http or an app scheme (e.g. cursor://).
      if (!/^[a-z][a-z0-9+.-]*:/i.test(redirect) || /^(javascript|data|vbscript):/i.test(redirect)) {
        throw new Error("The server did not say where to go next. Start again from the app.");
      }
      return redirect;
    },
  });
}
