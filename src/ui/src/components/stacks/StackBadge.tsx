import { Link } from "@tanstack/react-router";
import { Container } from "lucide-react";

import type { Service } from "../../api/types";

/** Services registered from a compose stack; the services API refuses changes to them (409). */
export const isStackService = (a: Pick<Service, "runtime"> | null | undefined) => a?.runtime === "compose-service";

/** "stack · <name>" chip linking to the stack the service comes from. */
export function StackBadge({ a }: { a: Pick<Service, "spec"> }) {
  const id = a.spec?.stack_id;
  const label = (
    <span className="inline-flex max-w-[200px] items-center gap-1 rounded bg-accent-soft px-1.5 py-0.5 text-xs font-medium text-accent">
      <Container className="size-3 shrink-0" />
      <span className="truncate">stack{a.spec?.stack ? ` · ${a.spec.stack}` : ""}</span>
    </span>
  );
  if (!id) return label;
  return (
    <Link to="/stacks/$stackId" params={{ stackId: id }} title={`Managed by stack ${a.spec?.stack ?? id}: change it there`} className="hover:opacity-80">
      {label}
    </Link>
  );
}
