import type { SwarmTask, SwarmTaskStat } from "../../api/swarm";
import { bytes } from "../ui";

export const isRunning = (t: SwarmTask) => t.state === "running";

/** Docker node ids of the running tasks, for `useSwarmTaskStats`. */
export const runningNodes = (tasks: SwarmTask[]) => tasks.filter(isRunning).map((t) => t.node_id ?? "");

/** One line per running task: slot, CPU % and memory (of its limit when set). */
export function TaskUsage({ tasks, stats }: { tasks: SwarmTask[]; stats?: Map<string, SwarmTaskStat> }) {
  const running = tasks.filter(isRunning);
  if (!running.length || !stats) return null;
  return (
    <div className="mt-0.5 space-y-px">
      {running.map((t) => {
        const s = stats.get(t.id);
        const mem = s?.memory_bytes === undefined ? "—" : `${bytes(s.memory_bytes)}${s.memory_limit ? ` / ${bytes(s.memory_limit)}` : ""}`;
        return (
          <div key={t.id} className="flex gap-2 font-mono text-[10.5px] text-faint tabular-nums" title={`Task ${t.id}`}>
            <span className="w-8 shrink-0">{t.slot !== undefined ? `#${t.slot}` : t.id.slice(0, 6)}</span>
            <span className="w-12 shrink-0">{s?.cpu_percent === undefined ? "—" : `${s.cpu_percent.toFixed(1)}%`}</span>
            <span className="truncate">{mem}</span>
          </div>
        );
      })}
    </div>
  );
}
