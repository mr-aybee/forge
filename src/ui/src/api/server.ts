// Server-level settings (D-120, D-167): the optional pinned address, the
// hosts the console answers on, and the
// effective forge.toml values it falls back to. Read: platform-scope roles;
// change: `system.update` on the platform with an MFA-verified session.
import { useQuery } from "@tanstack/react-query";

import { ApiError, api, get } from "./client";
import { useAction } from "./hooks";

/** Where the address links use comes from: pinned (setting or forge.toml), or the last administrator sign-in. */
export type AddressSource = "setting" | "config" | "last_admin" | "none";

export interface PublicAddressView {
  /** The address links use now. */
  effective: string;
  source: AddressSource;
  /** The runtime setting; null when not set. */
  setting: string | null;
  /** `public_url` from forge.toml; null when not set. */
  configured: string | null;
  /** The address an administrator last signed in on (read-only, recorded automatically). */
  last_admin: string | null;
  /** Links use HTTPS (from the effective address's scheme). */
  https: boolean;
  /** Browser origins the WebSocket and MCP Origin guards accept. */
  allowed_origins: string[];
}

/** The hosts the console answers on (`*` = any). */
export interface AllowedHostsView {
  hosts: string[];
  any: boolean;
}

export interface ServerConfigView {
  public_url: string | null;
  node_public_url: string;
  agent_public_url: string;
  cookie_secure: boolean;
  trusted_proxies: string[];
}

export interface ServerSettings {
  public_address: PublicAddressView;
  allowed_hosts: AllowedHostsView;
  config: ServerConfigView;
}

export const serverKeys = {
  all: ["server"] as const,
  settings: () => [...serverKeys.all, "settings"] as const,
};

export const useServerSettings = () =>
  useQuery({
    queryKey: serverKeys.settings(),
    queryFn: () => get<ServerSettings>("/api/v1/server/settings"),
    retry: (n, e) => !(e instanceof ApiError && e.status === 403) && n < 2,
    staleTime: 10_000,
  });

export const useSetPublicAddress = () =>
  useAction(
    (value: string) =>
      api<PublicAddressView>("PUT", "/api/v1/server/public-address", { value }),
    [serverKeys.all],
  );

export const useClearPublicAddress = () =>
  useAction(
    () => api<PublicAddressView>("DELETE", "/api/v1/server/public-address"),
    [serverKeys.all],
  );

export const useSetAllowedHosts = () =>
  useAction(
    (hosts: string[]) =>
      api<AllowedHostsView>("PUT", "/api/v1/server/allowed-hosts", { hosts }),
    [serverKeys.all],
  );

export const useResetAllowedHosts = () =>
  useAction(
    () => api<AllowedHostsView>("DELETE", "/api/v1/server/allowed-hosts"),
    [serverKeys.all],
  );
