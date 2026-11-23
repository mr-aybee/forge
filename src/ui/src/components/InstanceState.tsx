import type { ServiceInstance, Server } from "../api/types";
import { Badge } from "./ui";

/** Live state of one placement, derived from the node's agent status and the last state the node reported. */
export type LiveKind = "running" | "stopped" | "failed" | "changing" | "not-deployed" | "node-offline" | "unknown";

export interface LiveState {
  kind: LiveKind;
  label: string;
  tone: "ok" | "warn" | "bad" | "muted";
  /** Tooltip: why this state, from which data. */
  why: string;
}

const RUNNING = new Set(["active", "running", "activating", "reloading"]);
const STOPPED = new Set(["inactive", "exited", "stopped", "dead", "removed", "created", "paused"]);

/**
 * Combines agent status and instance state. Not a health check: it reports
 * whether the process/container was running when the node last told Forge.
 * `node` undefined means the node list hasn't loaded or the node is gone.
 */
export function liveState(i: Pick<ServiceInstance, "status" | "state" | "last_error"> | undefined | null, node: Pick<Server, "status"> | undefined | null): LiveState {
  if (!i) return { kind: "unknown", label: "unknown", tone: "muted", why: "No placement data" };
  const status = i.status ?? "";
  const state = (i.state ?? "").toLowerCase();
  if (node && (node.status === "offline" || node.status === "revoked"))
    return { kind: "node-offline", label: "node offline", tone: "bad", why: `The node's agent is ${node.status}; Forge can't tell whether it runs (last reported: ${i.state ?? "nothing"})` };
  if (status === "pending") return { kind: "not-deployed", label: "not deployed", tone: "muted", why: "Placed but not deployed to this node yet" };
  if (status.endsWith("ing")) return { kind: "changing", label: status, tone: "warn", why: `A job is in progress: ${status}` };
  if (status === "failed" && !state) return { kind: "failed", label: "failed", tone: "bad", why: i.last_error ?? "The last deployment failed" };
  if (RUNNING.has(state)) return { kind: "running", label: "running", tone: "ok", why: `Node reported "${i.state}"` };
  if (state === "failed") return { kind: "failed", label: "failed", tone: "bad", why: i.last_error ?? `Node reported "${i.state}"` };
  if (STOPPED.has(state)) return { kind: "stopped", label: "stopped", tone: "warn", why: `Node reported "${i.state}"` };
  if (status === "failed") return { kind: "failed", label: "failed", tone: "bad", why: i.last_error ?? "The last job failed" };
  return { kind: "unknown", label: state || "unknown", tone: "muted", why: state ? `Node reported "${i.state}"` : "The node hasn't reported a state yet" };
}

/** The State chip used on node, service and routing views. */
export function InstanceStateChip({ s }: { s: LiveState }) {
  return (
    <span title={s.why} className="inline-flex">
      <Badge tone={s.tone}>
        <span aria-hidden className={`mr-1.5 size-1.5 rounded-full bg-current ${s.kind === "changing" ? "animate-pulse" : ""}`} />
        {s.label}
      </Badge>
    </span>
  );
}
