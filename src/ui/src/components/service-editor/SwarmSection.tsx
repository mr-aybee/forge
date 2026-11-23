import { ChevronRight, Minus, Plus, Trash2 } from "lucide-react";
import { type ReactNode, useState } from "react";

import type { SwarmFailureAction, SwarmPublishMode, SwarmRestartCondition, SwarmServiceMode, SwarmSettings, SwarmUpdateOrder } from "../../api/swarm";
import { Button, IconButton, Input, Segmented, Select } from "../ui";
import { FormField, Grid, NumberInput } from "./parts";
import { CONSTRAINT_RE, SPREAD_RE } from "./sections";

/** Constraint keys Docker accepts; the two `labels` kinds take a label name. */
const KEYS = [
  { id: "node.role", label: "Node role", hint: "manager or worker" },
  { id: "node.hostname", label: "Node host name", hint: "node-1" },
  { id: "node.id", label: "Node id", hint: "swarm node id" },
  { id: "node.platform.os", label: "Node OS", hint: "linux" },
  { id: "node.platform.arch", label: "Node architecture", hint: "x86_64" },
  { id: "node.labels.", label: "Node label", hint: "value" },
  { id: "engine.labels.", label: "Engine label", hint: "value" },
] as const;

interface Row {
  key: string;
  label: string;
  op: "==" | "!=";
  value: string;
}

function parse(c: string): Row {
  const m = /^(.*?)\s*(==|!=)\s*(.*)$/.exec(c);
  const lhs = m?.[1] ?? c;
  const op = m?.[2] === "!=" ? "!=" : "==";
  const value = m?.[3] ?? "";
  const lbl = /^(node|engine)\.labels\.(.*)$/.exec(lhs);
  if (lbl) return { key: `${lbl[1]}.labels.`, label: lbl[2] ?? "", op, value };
  return { key: lhs || "node.role", label: "", op, value };
}

const format = (r: Row) => `${r.key}${r.key.endsWith(".labels.") ? r.label : ""}${r.op}${r.value}`;

let seq = 0;
const newId = () => `r${++seq}`;

/**
 * Stable row ids for a list edited here: kept in step with the list length,
 * so a row keeps its id (and focus) while its siblings are added or removed.
 */
function useRowIds(n: number) {
  const [ids, setIds] = useState<string[]>(() => Array.from({ length: n }, newId));
  // Lists replaced from outside (YAML, discard) get fresh ids for the extra rows.
  const current = ids.length === n ? ids : ids.length > n ? ids.slice(0, n) : [...ids, ...Array.from({ length: n - ids.length }, newId)];
  return {
    ids: current,
    add: () => setIds([...current, newId()]),
    remove: (i: number) => setIds(current.filter((_, j) => j !== i)),
  };
}

/** Docker Swarm settings of a container service in a Swarm cluster. */
export function SwarmSection({ value, onChange }: { value: SwarmSettings; onChange: (v: SwarmSettings) => void }) {
  const set = (p: Partial<SwarmSettings>) => onChange({ ...value, ...p });
  const global = value.mode === "global";
  return (
    <div className="space-y-4">
      <Grid>
        <FormField label="Mode" hint={global ? "One task on every node that matches the constraints." : "A fixed number of tasks, placed by the swarm."}>
          {() => (
            <Segmented<SwarmServiceMode>
              label="Mode"
              value={value.mode}
              onChange={(mode) => set({ mode })}
              options={[
                { id: "replicated", label: "Replicated" },
                { id: "global", label: "Global" },
              ]}
            />
          )}
        </FormField>
        {!global && (
          <FormField label="Replicas" hint="1 to 1000. Scale later from the service page without a redeploy." error={value.replicas < 1 || value.replicas > 1000 ? "1 to 1000." : undefined}>
            {(id, invalid) => (
              <div className="flex items-center gap-1">
                <IconButton type="button" label="One replica fewer" className="size-8" disabled={value.replicas <= 1} onClick={() => set({ replicas: Math.max(1, value.replicas - 1) })}>
                  <Minus className="size-4" />
                </IconButton>
                <NumberInput id={id} invalid={invalid} className="w-20 text-center" min={1} max={1000} value={value.replicas} onChange={(replicas) => set({ replicas })} />
                <IconButton type="button" label="One replica more" className="size-8" disabled={value.replicas >= 1000} onClick={() => set({ replicas: Math.min(1000, value.replicas + 1) })}>
                  <Plus className="size-4" />
                </IconButton>
              </div>
            )}
          </FormField>
        )}
        <FormField label="Published ports" hint={value.publish === "ingress" ? "Routing mesh: every node answers on the port and forwards to a task." : "Only nodes running a task listen; one task per node per port."}>
          {() => (
            <Segmented<SwarmPublishMode>
              label="Published ports"
              value={value.publish}
              onChange={(publish) => set({ publish })}
              options={[
                { id: "ingress", label: "Ingress mesh" },
                { id: "host", label: "Host" },
              ]}
            />
          )}
        </FormField>
        <FormField label="Max tasks per node" hint="Empty: no limit.">
          {(id) => <NumberInput id={id} min={0} placeholder="no limit" value={value.max_per_node} onChange={(max_per_node) => set({ max_per_node: Math.max(0, max_per_node) })} />}
        </FormField>
      </Grid>

      <Constraints value={value.constraints} onChange={(constraints) => set({ constraints })} />
      <Spread value={value.spread} onChange={(spread) => set({ spread })} />

      <Group title="Rollout" summary={`${value.update.parallelism || "all"} at a time · ${value.update.order} · on failure ${value.update.failure_action}`}>
        <Grid>
          <FormField label="Tasks at a time" hint="Empty: all at once.">
            {(id) => <NumberInput id={id} min={0} placeholder="all" value={value.update.parallelism} onChange={(parallelism) => set({ update: { ...value.update, parallelism: Math.max(0, parallelism) } })} />}
          </FormField>
          <FormField label="Delay between batches (s)">
            {(id) => <NumberInput id={id} min={0} placeholder="0" value={value.update.delay_secs} onChange={(delay_secs) => set({ update: { ...value.update, delay_secs: Math.max(0, delay_secs) } })} />}
          </FormField>
          <FormField label="Order" hint={value.update.order === "start-first" ? "Starts the new task before stopping the old one: no gap, briefly both run." : "Stops the old task first: a short gap per task."}>
            {(id) => (
              <Select id={id} value={value.update.order} onChange={(e) => set({ update: { ...value.update, order: e.target.value as SwarmUpdateOrder } })}>
                <option value="stop-first">Stop first</option>
                <option value="start-first">Start first</option>
              </Select>
            )}
          </FormField>
          <FormField label="When a batch fails">
            {(id) => (
              <Select id={id} value={value.update.failure_action} onChange={(e) => set({ update: { ...value.update, failure_action: e.target.value as SwarmFailureAction } })}>
                <option value="pause">Pause the rollout</option>
                <option value="rollback">Roll back</option>
                <option value="continue">Continue</option>
              </Select>
            )}
          </FormField>
          <FormField label="Watch each task for (s)" hint="A task that fails within this time counts as a failed update.">
            {(id) => <NumberInput id={id} min={0} placeholder="0" value={value.update.monitor_secs} onChange={(monitor_secs) => set({ update: { ...value.update, monitor_secs: Math.max(0, monitor_secs) } })} />}
          </FormField>
          <FormField label="Tolerated failure ratio" hint="0 to 1. 0: any failed task triggers the failure action." error={value.update.max_failure_ratio < 0 || value.update.max_failure_ratio > 1 ? "0 to 1." : undefined}>
            {(id, invalid) => <NumberInput id={id} invalid={invalid} min={0} max={1} step={0.05} placeholder="0" value={value.update.max_failure_ratio} onChange={(max_failure_ratio) => set({ update: { ...value.update, max_failure_ratio } })} />}
          </FormField>
        </Grid>
      </Group>

      <Group title="Restart policy" summary={`${value.restart.condition} · ${value.restart.max_attempts || "unlimited"} attempts · ${value.restart.delay_secs}s delay`}>
        <Grid>
          <FormField label="Restart when">
            {(id) => (
              <Select id={id} value={value.restart.condition} onChange={(e) => set({ restart: { ...value.restart, condition: e.target.value as SwarmRestartCondition } })}>
                <option value="any">Any exit</option>
                <option value="on-failure">Non-zero exit</option>
                <option value="none">Never</option>
              </Select>
            )}
          </FormField>
          <FormField label="Max attempts" hint="Empty: unlimited.">
            {(id) => <NumberInput id={id} min={0} placeholder="unlimited" value={value.restart.max_attempts} onChange={(max_attempts) => set({ restart: { ...value.restart, max_attempts: Math.max(0, max_attempts) } })} />}
          </FormField>
          <FormField label="Delay (s)">
            {(id) => <NumberInput id={id} min={0} placeholder="0" value={value.restart.delay_secs} onChange={(delay_secs) => set({ restart: { ...value.restart, delay_secs: Math.max(0, delay_secs) } })} />}
          </FormField>
        </Grid>
      </Group>
    </div>
  );
}

/** A collapsed group with a one-line summary of its values. */
function Group({ title, summary, children }: { title: string; summary: string; children: ReactNode }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="rounded-md border border-line">
      <button
        type="button"
        aria-expanded={open}
        onClick={() => setOpen((o) => !o)}
        className="flex w-full min-w-0 items-center gap-2 px-3 py-1.5 text-left focus-visible:outline-2 focus-visible:outline-accent"
      >
        <ChevronRight className={`size-3.5 shrink-0 text-muted transition-transform ${open ? "rotate-90" : ""}`} aria-hidden />
        <span className="text-[13px] font-medium">{title}</span>
        <span className="min-w-0 flex-1 truncate font-mono text-[11px] text-faint" title={summary}>
          {summary}
        </span>
      </button>
      {open && <div className="border-t border-line px-3 py-3">{children}</div>}
    </div>
  );
}

function Constraints({ value, onChange }: { value: string[]; onChange: (v: string[]) => void }) {
  const rows = useRowIds(value.length);
  const setRow = (i: number, p: Partial<Row>) => onChange(value.map((c, j) => (j === i ? format({ ...parse(c), ...p }) : c)));
  return (
    <div className="space-y-1.5">
      <p className="text-xs font-medium text-muted">Placement constraints</p>
      {value.length === 0 ? (
        <p className="rounded-md border border-dashed border-line px-3 py-2 text-xs text-faint">None: tasks may run on any active node of the swarm.</p>
      ) : (
        <ul className="divide-y divide-line rounded-md border border-line">
          {value.map((c, i) => {
            const r = parse(c);
            const k = KEYS.find((x) => x.id === r.key);
            const isLabel = r.key.endsWith(".labels.");
            const bad = !CONSTRAINT_RE.test(c);
            return (
              <li key={rows.ids[i]} className="flex flex-wrap items-center gap-1.5 px-2 py-1">
                <Select aria-label={`Constraint ${i + 1} key`} className="h-7 w-44 py-0 text-xs" value={r.key} onChange={(e) => setRow(i, { key: e.target.value, label: "" })}>
                  {!k && <option value={r.key}>{r.key}</option>}
                  {KEYS.map((x) => (
                    <option key={x.id} value={x.id}>
                      {x.label}
                    </option>
                  ))}
                </Select>
                {isLabel && (
                  <Input aria-label={`Constraint ${i + 1} label name`} className="h-7 w-32 py-0 font-mono text-xs" placeholder="zone" value={r.label} onChange={(e) => setRow(i, { label: e.target.value.trim() })} />
                )}
                <Select aria-label={`Constraint ${i + 1} operator`} className="h-7 w-16 py-0 font-mono text-xs" value={r.op} onChange={(e) => setRow(i, { op: e.target.value === "!=" ? "!=" : "==" })}>
                  <option value="==">==</option>
                  <option value="!=">!=</option>
                </Select>
                <Input
                  aria-label={`Constraint ${i + 1} value`}
                  aria-invalid={bad || undefined}
                  className={`h-7 min-w-24 flex-1 py-0 font-mono text-xs ${bad ? "border-bad" : ""}`}
                  placeholder={k?.hint ?? "value"}
                  value={r.value}
                  onChange={(e) => setRow(i, { value: e.target.value.trim() })}
                />
                <span className="hidden max-w-56 truncate font-mono text-[11px] text-faint sm:inline" title={c}>
                  {c}
                </span>
                <IconButton
                  type="button"
                  label={`Remove constraint ${i + 1}`}
                  className="size-7 hover:text-bad"
                  onClick={() => {
                    rows.remove(i);
                    onChange(value.filter((_, j) => j !== i));
                  }}
                >
                  <Trash2 className="size-3.5" />
                </IconButton>
              </li>
            );
          })}
        </ul>
      )}
      <Button
        type="button"
        variant="ghost"
        onClick={() => {
          rows.add();
          onChange([...value, "node.role==worker"]);
        }}
      >
        <Plus className="size-4" /> Add constraint
      </Button>
    </div>
  );
}

function Spread({ value, onChange }: { value: string[]; onChange: (v: string[]) => void }) {
  const rows = useRowIds(value.length);
  const name = (s: string) => s.replace(/^node\.labels\./, "");
  return (
    <div className="space-y-1.5">
      <p className="text-xs font-medium text-muted">Spread over node labels</p>
      {value.length === 0 ? (
        <p className="rounded-md border border-dashed border-line px-3 py-2 text-xs text-faint">None. Add a label such as zone to spread tasks evenly over its values.</p>
      ) : (
        <ul className="divide-y divide-line rounded-md border border-line">
          {value.map((s, i) => {
            const bad = !!s && !SPREAD_RE.test(s);
            return (
              <li key={rows.ids[i]} className="flex items-center gap-1.5 px-2 py-1">
                <span className="shrink-0 font-mono text-[11px] text-faint">node.labels.</span>
                <Input
                  aria-label={`Spread label ${i + 1}`}
                  aria-invalid={bad || undefined}
                  className={`h-7 py-0 font-mono text-xs ${bad ? "border-bad" : ""}`}
                  placeholder="zone"
                  value={name(s)}
                  onChange={(e) => onChange(value.map((x, j) => (j === i ? `node.labels.${e.target.value.trim()}` : x)))}
                />
                <IconButton
                  type="button"
                  label={`Remove spread label ${i + 1}`}
                  className="size-7 hover:text-bad"
                  onClick={() => {
                    rows.remove(i);
                    onChange(value.filter((_, j) => j !== i));
                  }}
                >
                  <Trash2 className="size-3.5" />
                </IconButton>
              </li>
            );
          })}
        </ul>
      )}
      <span title={value.length >= 4 ? "At most 4 spread labels" : undefined} className="inline-flex">
        <Button
          type="button"
          variant="ghost"
          disabled={value.length >= 4}
          onClick={() => {
            rows.add();
            onChange([...value, "node.labels.zone"]);
          }}
        >
          <Plus className="size-4" /> Add spread label
        </Button>
      </span>
    </div>
  );
}
