import { BellPlus } from "lucide-react";
import { useState } from "react";

import { Button } from "../ui";
import { RuleDialog, type RulePrefill } from "./RuleDialog";

/**
 * "Add alert" where a resource is shown (D-118): opens the rule dialog with
 * this place's templates and the resource as the scope. The dialog mounts
 * only while open, so each opening starts fresh.
 */
export function AddAlertButton({ prefill, label = "Add alert", className = "h-7 px-2 text-xs", title }: { prefill: RulePrefill; label?: string; className?: string; title?: string }) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <Button type="button" className={className} title={title ?? "Create an alert rule from a template"} onClick={() => setOpen(true)}>
        <BellPlus className="size-3.5" /> {label}
      </Button>
      {open && <RuleDialog rule={null} prefill={prefill} onClose={() => setOpen(false)} />}
    </>
  );
}
