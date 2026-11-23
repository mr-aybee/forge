// Review of staged access changes: each change as a sentence, publish changes
// with their dry-run impact, then Apply runs them one by one. Refused changes
// stay staged with the server's reason.

import { useState } from "react";
import { AlertTriangle, Check, Loader2, Minus, Plus, ShieldCheck, Undo2 } from "lucide-react";

import { usePublishImpact, useApplyAccessChanges, type AccessChange } from "../../api/accessControl";
import { errorText, useToast } from "../../app/toast";
import { Badge, Button, Modal } from "../ui";
import type { Staging } from "./model";

export interface ChangeText {
  title: string;
  detail: string;
}

function Impact({ c }: { c: Extract<AccessChange, { type: "publish" }> }) {
  const q = usePublishImpact(c.profile_id, c.actions, c.expected_revision);
  if (q.isPending) return <span className="text-[11px] text-faint">Checking impact…</span>;
  if (q.isError) return <span className="text-[11px] text-warn">Impact unavailable: {errorText(q.error)}</span>;
  const i = q.data;
  return (
    <span className="block text-[11px] text-muted">
      <span className="font-mono font-semibold text-grant-add tabular-nums">+{i.added.length}</span> <span className="font-mono font-semibold text-grant-remove tabular-nums">−{i.removed.length}</span> actions · affects{" "}
      <span className="font-mono tabular-nums">{i.assignments}</span> assignment{i.assignments === 1 ? "" : "s"}, <span className="font-mono tabular-nums">{i.users}</span> user
      {i.users === 1 ? "" : "s"}
      {i.blocked.length > 0 && (
        <span className="mt-0.5 flex items-start gap-1 text-grant-narrow">
          <AlertTriangle className="mt-px size-3 shrink-0" />
          {i.blocked.length} assignment{i.blocked.length === 1 ? "" : "s"} would grant actions you don't hold there ({[...new Set(i.blocked.flatMap((b) => b.missing))].slice(0, 4).join(", ")}); the server refuses this publish.
        </span>
      )}
    </span>
  );
}

export function ReviewDialog({ staging, describe, onClose }: { staging: Staging; describe: (c: AccessChange) => ChangeText; onClose: () => void }) {
  const apply = useApplyAccessChanges();
  const toast = useToast();
  const { changes, errors } = staging;
  const failed = changes.filter((c) => errors[c.key]).length;
  // Changes applied by the last run, shown as done until the dialog closes.
  const [applied, setApplied] = useState<ChangeText[]>([]);

  const run = () =>
    apply.mutate(changes, {
      onSuccess: (results) => {
        const okKeys = new Set(results.filter((r) => r.ok).map((r) => r.key));
        setApplied(changes.filter((c) => okKeys.has(c.key)).map(describe));
        staging.settle(results);
        const ok = results.filter((r) => r.ok).length;
        const bad = results.length - ok;
        if (bad === 0) {
          toast(`${ok} change${ok === 1 ? "" : "s"} applied`);
          onClose();
        } else toast(`${ok} applied, ${bad} refused; the refused ones stay staged`, "error");
      },
      onError: (e) => toast(errorText(e), "error"),
    });

  return (
    <Modal open onOpenChange={(o) => !o && !apply.isPending && onClose()} title="Review access changes" description={changes.length ? `${changes.length} change${changes.length === 1 ? "" : "s"}, not written yet. Apply runs them in this order.` : undefined} wide>
      {applied.length > 0 && (
        <ul className="mb-2 space-y-0.5 text-xs">
          {applied.map((t, i) => (
            <li key={i} className="flex min-w-0 items-center gap-1.5 text-grant">
              <Check className="size-3.5 shrink-0" />
              <span className="truncate" title={t.title}>
                {t.title}
              </span>
              <span className="shrink-0 text-faint">applied</span>
            </li>
          ))}
        </ul>
      )}
      {changes.length === 0 ? (
        <p className="py-6 text-center text-muted">Nothing is staged. Add or remove a role, a member or a permission to compose a change.</p>
      ) : (
        <>
          {failed > 0 && <p className="mb-2 rounded-md border border-bad/40 bg-bad-soft px-2.5 py-1.5 text-xs font-medium text-bad">{failed} change{failed === 1 ? " was" : "s were"} refused when last applied. Fix or drop them.</p>}
          <ul className="flex flex-col gap-1.5">
            {changes.map((c) => {
              const t = describe(c);
              const err = errors[c.key];
              const remove = c.type === "unassign" || (c.type === "member" && !c.add);
              const Icon = c.type === "publish" ? ShieldCheck : remove ? Minus : Plus;
              return (
                <li key={c.key} className={`flex items-start gap-2.5 rounded-md border px-2.5 py-2 ${err ? "border-bad bg-bad-soft" : "border-line bg-panel-2"}`}>
                  <span className={`mt-0.5 grid size-6 shrink-0 place-items-center rounded-md ${remove ? "bg-grant-remove-soft text-grant-remove" : c.type === "publish" ? "bg-accent-soft text-accent" : "bg-grant-add-soft text-grant-add"}`}>
                    <Icon className="size-3.5" />
                  </span>
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-[13px] font-semibold" title={t.title}>
                      {t.title}
                    </span>
                    <span className="block text-xs text-muted">{t.detail}</span>
                    {c.type === "publish" && <Impact c={c} />}
                    {err && <span className="mt-0.5 block text-xs break-words text-bad">{err}</span>}
                  </span>
                  {err && <Badge tone="bad">refused</Badge>}
                  <button type="button" onClick={() => staging.unstage(c.key)} disabled={apply.isPending} className="shrink-0 rounded p-1 text-faint hover:bg-panel hover:text-fg disabled:opacity-40" aria-label={`Drop: ${t.title}`} title="Drop this change">
                    <Undo2 className="size-3.5" />
                  </button>
                </li>
              );
            })}
          </ul>
        </>
      )}
      <div className="mt-4 flex flex-wrap justify-end gap-2">
        <Button onClick={onClose} disabled={apply.isPending}>
          Continue editing
        </Button>
        <Button variant="danger" onClick={staging.clear} disabled={!changes.length || apply.isPending}>
          Discard all
        </Button>
        <Button variant="primary" onClick={run} disabled={!changes.length || apply.isPending}>
          {apply.isPending ? <Loader2 className="size-4 animate-spin" /> : <Check className="size-4" />} Apply {changes.length} change{changes.length === 1 ? "" : "s"}
        </Button>
      </div>
    </Modal>
  );
}
