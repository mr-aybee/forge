import type { ReactNode } from "react";

import { useMyCapabilities } from "../api/access";
import { ErrorNote, NoAccess, Skeleton } from "../components/ui";
import { capAllowed, gateOf } from "./nav";

/**
 * Renders the page at `path` only when the user's capabilities include its
 * navigation gate; otherwise a clear "no access" state instead of failing
 * requests and empty tables. Ungated pages render at once. The server still
 * refuses what it refuses; this only spares the user a broken page.
 */
export function RouteGate({ path, children }: { path: string; children: ReactNode }) {
  const gate = gateOf(path);
  const caps = useMyCapabilities();
  if (!gate) return children;
  if (caps.isPending) return <Skeleton />;
  if (caps.isError) return <ErrorNote error={caps.error} />;
  return capAllowed(gate, caps.data) ? children : <NoAccess />;
}
