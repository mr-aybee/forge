import { Ban, Loader2 } from "lucide-react";
import { useState } from "react";

import { useCancelJob } from "../api/hooks";
import type { Job } from "../api/types";
import { errorText, useToast } from "../app/toast";
import { isActive } from "./jobMeta";
import { Button, Confirm } from "./ui";

/**
 * Cancel for a queued or running job, behind a confirmation. Renders nothing
 * for finished jobs. `compact` is a small icon button for list rows.
 */
export function CancelJobButton({ job, title, compact }: { job: Pick<Job, "id" | "status">; title?: string; compact?: boolean }) {
  const cancel = useCancelJob();
  const toast = useToast();
  const [open, setOpen] = useState(false);
  if (!isActive(job)) return null;
  const busy = cancel.isPending;
  return (
    <>
      {compact ? (
        <button
          type="button"
          aria-label={`Cancel ${title ?? "job"}`}
          title="Cancel job"
          disabled={busy}
          onClick={(e) => {
            e.stopPropagation();
            setOpen(true);
          }}
          className="inline-grid size-7 place-items-center rounded-md text-muted transition hover:bg-bad-soft hover:text-bad disabled:opacity-50"
        >
          {busy ? <Loader2 className="size-3.5 animate-spin" /> : <Ban className="size-3.5" />}
        </button>
      ) : (
        <Button variant="danger" disabled={busy} onClick={() => setOpen(true)}>
          {busy ? <Loader2 className="size-4 animate-spin" /> : <Ban className="size-4" />} Cancel job
        </Button>
      )}
      <Confirm
        open={open}
        onOpenChange={setOpen}
        title="Cancel this job?"
        body={
          <>
            {title && <p className="mb-2 font-medium text-fg">{title}</p>}
            <p>Steps already done stay done; the running step is stopped.</p>
          </>
        }
        confirmLabel="Cancel job"
        cancelLabel="Keep running"
        busy={busy}
        onConfirm={() =>
          cancel.mutate(job.id, {
            onSuccess: () => {
              toast("Job cancelled");
              setOpen(false);
            },
            onError: (e) => {
              toast(errorText(e), "error");
              setOpen(false);
            },
          })
        }
      />
    </>
  );
}
