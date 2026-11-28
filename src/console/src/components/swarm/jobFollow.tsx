import { useState, type ReactNode } from "react";

import type { Job } from "../../api/types";
import { useToast } from "../../app/toast";
import { JobDetail } from "../JobsDrawer";
import { Drawer } from "../ui";

/**
 * Shows a started job: a toast with "Follow job" that opens its live log in a
 * drawer. Render `view` once in the component.
 */
export function useJobFollow(title = "Swarm job") {
  const toast = useToast();
  const [job, setJob] = useState<string>();
  const started = (j: Job | undefined, text: string) => {
    if (j?.id) toast(text, "ok", { label: "Follow job", onSelect: () => setJob(j.id) });
    else toast(text);
  };
  const view: ReactNode = job ? (
    <Drawer open onOpenChange={(o) => !o && setJob(undefined)} title={title} wide>
      <JobDetail id={job} onNavigate={() => setJob(undefined)} />
    </Drawer>
  ) : null;
  return { started, view };
}
