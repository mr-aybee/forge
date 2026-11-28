import { ClipboardList, Loader2, Rocket } from "lucide-react";
import { useState } from "react";

import { useApplyServicesFile, usePlanServicesFile } from "../../api/hooks";
import type { ServicesPlan } from "../../api/types";
import { errorText, useToast } from "../../app/toast";
import { Badge, Button, Panel, TextArea } from "../ui";

const tone = { create: "ok", update: "warn", move: "warn", remove: "bad", unchanged: "muted" } as const;

/**
 * A Compose-format services file (D-178): edit the YAML, **Plan** shows what
 * would change, **Apply** runs it. Never prunes: other services of the file
 * stay. Mount it with a `key` so a new `initial` resets the text.
 */
export function ServicesFilePanel({ environmentId, initial, onApplied }: { environmentId?: string; initial: string; onApplied?: () => void }) {
  const [state, setState] = useState<{ text: string; plan: ServicesPlan | null; error: string }>({ text: initial, plan: null, error: "" });
  const plan = usePlanServicesFile();
  const apply = useApplyServicesFile();
  const toast = useToast();
  const busy = plan.isPending || apply.isPending;
  const body = () => ({ environment_id: environmentId ?? "", file: state.text, prune: false });

  const runPlan = async () => {
    try {
      const p = await plan.mutateAsync(body());
      setState((s) => ({ ...s, plan: p, error: "" }));
    } catch (e) {
      setState((s) => ({ ...s, plan: null, error: errorText(e) }));
    }
  };

  const runApply = async () => {
    try {
      const r = await apply.mutateAsync(body());
      setState((s) => ({ ...s, plan: r, error: "" }));
      toast(`Applied; ${r.jobs.length} job${r.jobs.length === 1 ? "" : "s"} started`);
      onApplied?.();
    } catch (e) {
      setState((s) => ({ ...s, error: errorText(e) }));
    }
  };

  return (
    <Panel title="Services file (Compose format)">
      <div className="space-y-3 p-3">
        <TextArea
          aria-label="Services file YAML"
          spellCheck={false}
          rows={18}
          className="font-mono text-[12px]"
          value={state.text}
          onChange={(e) => setState({ text: e.target.value, plan: null, error: "" })}
        />
        {state.error && (
          <p role="alert" className="rounded-md border border-bad bg-bad-soft px-3 py-2 text-[13px] break-words text-bad">
            {state.error}
          </p>
        )}
        {state.plan && (
          <ul className="space-y-1 text-[13px]">
            {state.plan.changes.map((c) => (
              <li key={c.service} className="flex flex-wrap items-center gap-2">
                <Badge tone={tone[c.action]}>{c.action}</Badge>
                <b>{c.service}</b>
                <span className="text-muted">on {c.nodes.join(", ") || "—"}</span>
                {c.fields.length > 0 && <span className="text-muted">changes {c.fields.join(", ")}</span>}
                {c.added_nodes.length > 0 && <span className="text-muted">adds {c.added_nodes.join(", ")}</span>}
                {c.removed_nodes.length > 0 && <span className="text-muted">takes off {c.removed_nodes.join(", ")}</span>}
              </li>
            ))}
          </ul>
        )}
        <div className="flex justify-end gap-2">
          <Button type="button" disabled={!environmentId || busy} onClick={() => void runPlan()} title={environmentId ? undefined : "Choose an environment first"}>
            {plan.isPending ? <Loader2 className="size-4 animate-spin" /> : <ClipboardList className="size-4" />} Plan
          </Button>
          <Button type="button" variant="primary" disabled={!environmentId || busy || !state.plan} onClick={() => void runApply()} title={state.plan ? undefined : "Plan first"}>
            {apply.isPending ? <Loader2 className="size-4 animate-spin" /> : <Rocket className="size-4" />} Apply
          </Button>
        </div>
      </div>
    </Panel>
  );
}
