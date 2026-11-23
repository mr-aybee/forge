// Server-level settings (D-120): the console's public address and the
// effective forge.toml values it falls back to. Read: platform-scope roles;
// change: `system.update` on the platform with an MFA-verified session.
import { useQuery } from "@tanstack/react-query";

import { ApiError, api, get } from "./client";
import { useAction } from "./hooks";

export type AddressSource = "setting" | "config";

export interface PublicAddressView {
  /** The address links use now. */
  effective: string;
  source: AddressSource;
  /** The runtime setting; null when not set. */
  setting: string | null;
  /** `public_url` from forge.toml (the fallback). */
  configured: string;
  /** Links use HTTPS (from the effective address's scheme). */
  https: boolean;
  /** Browser origins the WebSocket and MCP Origin guards accept. */
  allowed_origins: string[];
}

export interface ServerConfigView {
  public_url: string;
  node_public_url: string;
  agent_public_url: string;
  cookie_secure: boolean;
  trusted_proxies: string[];
}

export interface ServerSettings {
  public_address: PublicAddressView;
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
  useAction((value: string) => api<PublicAddressView>("PUT", "/api/v1/server/public-address", { value }), [serverKeys.all]);

export const useClearPublicAddress = () =>
  useAction(() => api<PublicAddressView>("DELETE", "/api/v1/server/public-address"), [serverKeys.all]);
