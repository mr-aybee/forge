// Console access: which areas the signed-in user may open
// (`GET /api/v1/me/capabilities`). Navigation hides what the server would
// refuse; the server still authorizes every request.

import { useQuery } from "@tanstack/react-query";

import { get } from "./client";

/** One flag per gated navigation entry; see the REST reference. */
export interface ConsoleCapabilities {
  fleet_view: boolean;
  terminal: boolean;
  package_install: boolean;
  software_updates: boolean;
  sftp_access: boolean;
  tunnels: boolean;
  alerts_view: boolean;
  alerts_manage: boolean;
  notifications_manage: boolean;
  audit_view: boolean;
  users_manage: boolean;
  access_manage: boolean;
  vault: boolean;
  forge_updates: boolean;
  /** Change analytics settings and restart the managed engine (`analytics.configure`). */
  analytics_configure: boolean;
  plugins: boolean;
  platform_settings: boolean;
}

export type Capability = keyof ConsoleCapabilities;

const FIELDS: Capability[] = [
  "fleet_view",
  "terminal",
  "package_install",
  "software_updates",
  "sftp_access",
  "tunnels",
  "alerts_view",
  "alerts_manage",
  "notifications_manage",
  "audit_view",
  "users_manage",
  "access_manage",
  "vault",
  "forge_updates",
  "analytics_configure",
  "plugins",
  "platform_settings",
];

/** Nothing allowed: the state before the answer arrives. */
export const NONE = (): ConsoleCapabilities => ({
  fleet_view: false,
  terminal: false,
  package_install: false,
  software_updates: false,
  sftp_access: false,
  tunnels: false,
  alerts_view: false,
  alerts_manage: false,
  notifications_manage: false,
  audit_view: false,
  users_manage: false,
  access_manage: false,
  vault: false,
  forge_updates: false,
  analytics_configure: false,
  plugins: false,
  platform_settings: false,
});

/** Query keys. Under `["me"]`, so signing in, stepping up or a profile change refreshes them too. */
export const accessKeys = {
  capabilities: ["me", "capabilities"] as const,
};

/** A missing or non-boolean field (older server) counts as not allowed. */
function toCapabilities(x: unknown): ConsoleCapabilities {
  const o = x && typeof x === "object" ? (x as Record<string, unknown>) : {};
  const out: Record<Capability, boolean> = NONE();
  for (const f of FIELDS) out[f] = o[f] === true;
  return out;
}

/** The signed-in user's console capabilities. */
export const useMyCapabilities = () =>
  useQuery({
    queryKey: accessKeys.capabilities,
    queryFn: async () => toCapabilities(await get<unknown>("/api/v1/me/capabilities")),
    staleTime: 60_000,
  });
