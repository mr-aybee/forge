// Credential vault: the item list (metadata only; passwords are revealed on request and never cached).
import { useQuery } from "@tanstack/react-query";

import { get } from "./client";

export interface VaultItem {
  id: string;
  name: string;
  username: string;
  url: string;
  notes: string;
  tags: string[];
  links: { kind: "node" | "service" | "cluster"; id: string; name: string }[];
  created_at: number;
  updated_at: number;
  rotated_at: number;
}

export const vaultKeys = { items: ["vault-items"] as const };

/**
 * The vault items. Platform administrators only, with an MFA-verified
 * sign-in: everyone else gets a 403, which callers must show as such
 * (never as an empty vault).
 */
export const useVaultItems = (enabled = true) =>
  useQuery({ queryKey: vaultKeys.items, queryFn: () => get<VaultItem[]>("/api/v1/vault/items"), retry: false, enabled });
