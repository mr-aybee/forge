import { CheckCircle2, Loader2, RefreshCw, XCircle } from "lucide-react";

import { useConvertCheck, useConvertCluster } from "../../api/swarm";
import type { ClusterMode } from "../../api/types";
import { errorText, useToast } from "../../app/toast";
import { Button, ErrorNote, Modal, Skeleton } from "../ui";

/** Runs the server's conversion check, lists it, and converts only when every check passes. */
export function ConvertDialog({ clusterId, clusterName, to, onClose }: { clusterId: string; clusterName: string; to: ClusterMode; onClose: () => void }) {
  const check = useConvertCheck(clusterId, to);
  const convert = useConvertCluster(clusterId);
  const toast = useToast();
  const r = check.data;
  const target = to === "swarm" ? "Swarm" : "Default";

  return (
    <Modal open onOpenChange={(o) => !o && onClose()} title={`Convert ${clusterName} to ${target}`} description="Forge checks the cluster first. Nothing changes until you convert.">
      <div className="space-y-3 text-[13px]">
        <div>
          <div className="mb-1 text-xs font-medium text-muted">What happens</div>
          <ol className="list-decimal space-y-0.5 pl-5 text-xs text-muted">
            {to === "swarm" ? (
              <>
                <li>The cluster's mode becomes Swarm and a Swarm tab appears.</li>
                <li>No swarm is created yet: create it from the Swarm tab by picking the first manager.</li>
                <li>Existing services keep running as they are; new stacks in its environments deploy to the swarm.</li>
              </>
            ) : (
              <>
                <li>The cluster's mode becomes Default; Swarm management is hidden.</li>
                <li>Stacks deploy per node again.</li>
              </>
            )}
          </ol>
        </div>
        {check.error ? (
          <ErrorNote error={check.error} />
        ) : check.isLoading || !r ? (
          <Skeleton rows={3} />
        ) : (
          <ul className="divide-y divide-line rounded-md border border-line">
            {r.checks.length === 0 && <li className="px-2 py-1.5 text-xs text-muted">The server reported no checks.</li>}
            {r.checks.map((c) => (
              <li key={c.name} className="flex min-w-0 items-start gap-2 px-2 py-1.5">
                {c.ok ? <CheckCircle2 className="mt-0.5 size-3.5 shrink-0 text-ok" aria-label="Passed" /> : <XCircle className="mt-0.5 size-3.5 shrink-0 text-bad" aria-label="Failed" />}
                <div className="min-w-0">
                  <div className="font-medium">{c.name}</div>
                  {c.detail && <div className={`text-xs break-words ${c.ok ? "text-muted" : "text-bad"}`}>{c.detail}</div>}
                </div>
              </li>
            ))}
          </ul>
        )}
        {r && !r.ok && <p className="text-xs text-warn">Fix the failed checks, then check again.</p>}
      </div>
      <div className="mt-5 flex justify-end gap-2">
        <Button onClick={() => void check.refetch()} disabled={check.isFetching}>
          {check.isFetching ? <Loader2 className="size-4 animate-spin" /> : <RefreshCw className="size-4" />} Check again
        </Button>
        <Button
          variant="primary"
          disabled={!r?.ok || convert.isPending || check.isFetching}
          title={r?.ok ? undefined : "Every check must pass"}
          onClick={() =>
            convert.mutate(to, {
              onSuccess: () => {
                toast(`${clusterName} is now a ${target} cluster`);
                onClose();
              },
              onError: (e) => toast(errorText(e), "error"),
            })
          }
        >
          {convert.isPending && <Loader2 className="size-4 animate-spin" />} Convert to {target}
        </Button>
      </div>
    </Modal>
  );
}
