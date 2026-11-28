import { Check, CircleAlert, CircleDashed } from "lucide-react";

import type { SectionStatus } from "./sections";

const MARK = {
  complete: { icon: Check, cls: "text-ok", label: "Complete" },
  needs: { icon: CircleDashed, cls: "text-warn", label: "Needs input" },
  error: { icon: CircleAlert, cls: "text-bad", label: "Has an error" },
} as const;

/** Rail and card marker: complete, needs input, or error. */
export function StatusMark({ status }: { status: SectionStatus }) {
  const m = MARK[status];
  const Icon = m.icon;
  return (
    <span role="img" aria-label={m.label} title={m.label} className={`inline-flex shrink-0 ${m.cls}`}>
      <Icon className="size-3.5" aria-hidden />
    </span>
  );
}
