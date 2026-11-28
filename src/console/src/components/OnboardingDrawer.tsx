import { Link } from "@tanstack/react-router";

import type { Cluster, OnboardingNode } from "../api/types";
import { TokenConsole } from "./Console";
import { JobDetail } from "./JobsDrawer";
import { Badge, CopyValue, Drawer, Status, ago, time } from "./ui";

/** Display name of an onboarding entry: hostname, else the SSH host, else its id. */
export const onboardingName = (o: OnboardingNode) => o.hostname || o.host || (o.kind === "manual" ? "Manual install" : "New node");

/** `installing` while active, else the server's status (`failed`, `succeeded`…). */
export const onboardingStatus = (o: OnboardingNode) => (o.active ? "installing" : o.status);

/** One node being onboarded: its console, steps and error; Cancel for an active assisted job. */
export function OnboardingDrawer({ entry, clusters, onOpenChange }: { entry?: OnboardingNode; clusters: Cluster[]; onOpenChange: (o: boolean) => void }) {
  const cluster = entry?.cluster_id ? clusters.find((c) => c.id === entry.cluster_id) : undefined;
  return (
    <Drawer open={!!entry} onOpenChange={onOpenChange} title={entry ? `Onboarding ${onboardingName(entry)}` : "Onboarding"} wide>
      {entry && (
        <div className="space-y-4">
          <dl className="grid grid-cols-2 gap-x-4 gap-y-2 text-xs sm:grid-cols-4">
            <div className="min-w-0">
              <dt className="text-[11px] text-faint">Method</dt>
              <dd>
                <Badge>{entry.kind === "manual" ? "Manual installer" : "Assisted (SSH)"}</Badge>
              </dd>
            </div>
            <div className="min-w-0">
              <dt className="text-[11px] text-faint">Host</dt>
              <dd className="flex min-w-0 items-center gap-1">
                <CopyValue text={entry.host ?? entry.hostname ?? ""} label="Host" className="text-[11px]" />
              </dd>
            </div>
            <div className="min-w-0">
              <dt className="text-[11px] text-faint">Cluster</dt>
              <dd className="truncate">
                {entry.cluster_id ? (
                  <Link to="/clusters/$clusterId" params={{ clusterId: entry.cluster_id }} className="hover:text-accent">
                    {cluster?.name ?? entry.cluster_id.slice(0, 8)}
                  </Link>
                ) : (
                  <span className="text-faint">None</span>
                )}
              </dd>
            </div>
            <div className="min-w-0">
              <dt className="text-[11px] text-faint">Status</dt>
              <dd className="flex items-center gap-1.5">
                <Status value={onboardingStatus(entry)} />
                <span className="text-faint" title={time(entry.updated_at)}>
                  {entry.updated_at ? ago(entry.updated_at) : ""}
                </span>
              </dd>
            </div>
          </dl>
          {entry.server_id && (
            <p className="text-xs text-muted">
              Enrolled as{" "}
              <Link to="/nodes/$nodeId" params={{ nodeId: entry.server_id }} className="font-mono text-[11px] hover:text-accent">
                {entry.server_id.slice(0, 8)}
              </Link>
              .
            </p>
          )}
          {entry.job_id ? (
            <JobDetail id={entry.job_id} consoleHeight="h-96" onNavigate={() => onOpenChange(false)} />
          ) : entry.token_id ? (
            <div className="space-y-2">
              {entry.status === "failed" && (
                <div role="alert" className="rounded-md border border-bad/40 bg-bad-soft px-3 py-2 font-mono text-[11.5px] break-words whitespace-pre-wrap text-bad">
                  {entry.error ?? "The installer reported a failure. The console below shows the last lines."}
                </div>
              )}
              <TokenConsole tokenId={entry.token_id} height="h-96" />
              <p className="text-[11px] text-faint">
                Manual installs run on the machine itself; stop them there. The enrollment token expires on its own.
              </p>
            </div>
          ) : null}
        </div>
      )}
    </Drawer>
  );
}
