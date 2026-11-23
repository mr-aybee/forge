import { Link } from "@tanstack/react-router";
import { AppWindow, Layers } from "lucide-react";

import type { ContainerOwner } from "./owner";

/** "service → stack" links for a container's owner; `—` when Forge doesn't manage it. */
export function OwnerLink({ owner, compact }: { owner: ContainerOwner; compact?: boolean }) {
  if (!owner) return <span className="text-xs text-faint" title="Not created by Forge (no service or stack owns it)">—</span>;
  if (owner.kind === "stack") {
    return (
      <span className="inline-flex min-w-0 items-center gap-1 text-xs">
        <Layers className="size-3.5 shrink-0 text-faint" />
        <Link to="/stacks/$stackId" params={{ stackId: owner.id }} className="truncate text-accent hover:underline" title={`Stack ${owner.name}`}>
          {owner.name}
        </Link>
        {owner.service && !compact && <span className="truncate font-mono text-[11px] text-muted">/ {owner.service}</span>}
      </span>
    );
  }
  return (
    <span className="inline-flex min-w-0 items-center gap-1 text-xs">
      <AppWindow className="size-3.5 shrink-0 text-faint" />
      <Link to="/services/$serviceId" params={{ serviceId: owner.id }} className="truncate text-accent hover:underline" title={`Service ${owner.name}`}>
        {owner.name}
      </Link>
      {owner.stackId && !compact && (
        <>
          <span className="text-faint">←</span>
          <Link to="/stacks/$stackId" params={{ stackId: owner.stackId }} className="truncate text-muted hover:text-accent" title={`Stack ${owner.stackName ?? ""}`}>
            {owner.stackName ?? "stack"}
          </Link>
        </>
      )}
    </span>
  );
}
