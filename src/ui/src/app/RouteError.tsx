import { useEffect } from "react";

import { Button } from "../components/ui";
import { isChunkError, reloadForNewBuild } from "./staleBuild";

/** Route error screen; a stale build reloads itself instead of failing. */
export function RouteError({ error }: { error: unknown }) {
  const stale = isChunkError(error);
  useEffect(() => {
    if (stale) reloadForNewBuild();
  }, [stale]);
  return (
    <div className="mx-auto mt-16 max-w-lg rounded-lg border border-line bg-panel p-6 text-center shadow-[var(--shadow-card)]">
      <h2 className="text-lg font-semibold">{stale ? "Forge was updated" : "Something went wrong"}</h2>
      <p className="mt-2 text-sm text-muted">
        {stale ? "This page belongs to the previous version of the console. Reload to continue." : error instanceof Error ? error.message : String(error)}
      </p>
      <Button variant="primary" className="mt-4" onClick={() => window.location.reload()}>
        Reload
      </Button>
    </div>
  );
}
