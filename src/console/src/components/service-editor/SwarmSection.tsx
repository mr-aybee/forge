import { ChevronRight, Minus, Plus, X } from "lucide-react";
import { type ReactNode, useState } from "react";

import type { SwarmFailureAction, SwarmPublishMode, SwarmRestartCondition, SwarmServiceMode, SwarmSettings, SwarmUpdateOrder } from "../../api/swarm";
import { Button, IconButton, Input, Segmented, Select } from "../ui";
import { FormField, Grid, NumberInput } from "./parts";
import { SwarmFiles } from "./SwarmFiles";
import { CONSTRAINT_RE, DOCKER_NAME_RE, SPREAD_RE } from "./sections";

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

const slug = (s: string) => s.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "");

const FAIL: Record<SwarmFailureAction, string> = { pause: "pauses on failure", rollback: "rolls back on failure", continue: "continues on failure" };
const RESTART: Record<SwarmRestartCondition, string> = { any: "on any exit", "on-failure": "on non-zero exit", none: "never" };

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
export function SwarmSection({ value, onChange, serviceName = "", environmentId }: { value: SwarmSettings; onChange: (v: SwarmSettings) => void; serviceName?: string; environmentId?: string }) {
  const set = (p: Partial<SwarmSettings>) => onChange({ ...value, ...p });
  const global = value.mode === "global";
  const badReplicas = value.replicas < 1 || value.replicas > 1000;
  return (
    <div className="space-y-5">
      <Field
        label="Docker service name"
        htmlFor="swarm-name"
        hint="Containers show as <name>.<replica>.<task>. Used when the service is first created: Docker can't rename a running service."
        error={value.name && !DOCKER_NAME_RE.test(value.name) ? "Lowercase letters, digits and -, up to 61 characters." : undefined}
      >
        <div className="w-80 max-w-full">
          <Input
            id="swarm-name"
            className="font-mono text-xs"
            placeholder={`<environment>-${slug(serviceName) || "<service>"}`}
            value={value.name}
            onChange={(e) => set({ name: e.target.value.toLowerCase().replace(/[^a-z0-9-]/g, "-") })}
          />
        </div>
      </Field>

      {/* Scheduling: four compact controls in one strip. */}
      <div className="grid gap-x-6 gap-y-3 sm:grid-cols-2 xl:grid-cols-4">
        <Field label="Mode" hint={global ? "One task on every matching node." : "A fixed number of tasks."}>
          <Segmented<SwarmServiceMode>
            label="Mode"
            value={value.mode}
            onChange={(mode) => set({ mode })}
            options={[
              { id: "replicated", label: "Replicated" },
              { id: "global", label: "Global" },
            ]}
          />
        </Field>
        <Field label="Replicas" htmlFor="swarm-replicas" hint={global ? "Set by the number of matching nodes." : "1 to 1000; scale later without a redeploy."} error={!global && badReplicas ? "1 to 1000." : undefined}>
          {global ? (
            <div className="inline-flex h-8 items-center rounded-md bg-panel-2 px-3 text-xs text-muted">One per node</div>
          ) : (
            <div className="inline-flex h-8 items-stretch overflow-hidden rounded-md border border-line bg-panel">
              <IconButton type="button" label="One replica fewer" className="h-full w-8 rounded-none" disabled={value.replicas <= 1} onClick={() => set({ replicas: Math.max(1, value.replicas - 1) })}>
                <Minus className="size-3.5" />
              </IconButton>
              <div className="w-16 border-x border-line [&>input]:h-full [&>input]:rounded-none [&>input]:border-0 [&>input]:text-center">
                <NumberInput id="swarm-replicas" invalid={badReplicas} min={1} max={1000} value={value.replicas} onChange={(replicas) => set({ replicas })} />
              </div>
              <IconButton type="button" label="One replica more" className="h-full w-8 rounded-none" disabled={value.replicas >= 1000} onClick={() => set({ replicas: Math.min(1000, value.replicas + 1) })}>
                <Plus className="size-3.5" />
              </IconButton>
            </div>
          )}
        </Field>
        <Field label="Max tasks per node" htmlFor="swarm-max" hint="Empty: no limit.">
          <div className="w-28">
            <NumberInput id="swarm-max" min={0} placeholder="no limit" value={value.max_per_node} onChange={(max_per_node) => set({ max_per_node: Math.max(0, max_per_node) })} />
          </div>
        </Field>
        <Field label="Published ports" hint={value.publish === "ingress" ? "Every node answers and forwards to a task." : "Only nodes running a task listen."}>
          <Segmented<SwarmPublishMode>
            label="Published ports"
            value={value.publish}
            onChange={(publish) => set({ publish })}
            options={[
              { id: "ingress", label: "Ingress mesh" },
              { id: "host", label: "Host" },
            ]}
          />
        </Field>
      </div>

      {/* Placement: where tasks may run and how they spread. */}
      <div className="divide-y divide-line rounded-md border border-line">
        <Constraints value={value.constraints} onChange={(constraints) => set({ constraints })} />
        <Spread value={value.spread} onChange={(spread) => set({ spread })} />
      </div>

      <SwarmFiles value={value} onChange={set} environmentId={environmentId} />

      <div className="grid items-start gap-3 lg:grid-cols-2">
      <Group title="Rollout" summary={`${value.update.parallelism || "all"} at a time · ${value.update.order.replace("-", " ")} · ${FAIL[value.update.failure_action]}`}>
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

      <Group title="Restart policy" summary={`${RESTART[value.restart.condition]} · ${value.restart.max_attempts || "unlimited"} tries · ${value.restart.delay_secs}s apart`}>
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
    </div>
  );
}

/** A label above one control, with a hint or an error below. */
function Field({ label, htmlFor, hint, error, children }: { label: string; htmlFor?: string; hint?: string; error?: string; children: ReactNode }) {
  return (
    <div className="min-w-0 space-y-1">
      <label htmlFor={htmlFor} className="block text-xs font-medium text-muted">
        {label}
      </label>
      {children}
      {error ? (
        <p className="text-xs text-bad" role="alert">
          {error}
        </p>
      ) : (
        hint && <p className="text-xs text-faint">{hint}</p>
      )}
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
        className={`card-head flex w-full min-w-0 items-center gap-2 px-3 py-2 text-left focus-visible:outline-2 focus-visible:outline-accent ${open ? "rounded-t-md" : "rounded-md"}`}
      >
        <ChevronRight className={`size-3.5 shrink-0 text-muted transition-transform ${open ? "rotate-90" : ""}`} aria-hidden />
        <span className="text-[13px] font-medium">{title}</span>
        <span className="min-w-0 flex-1 truncate text-right text-xs text-faint" title={summary}>
          {summary}
        </span>
      </button>
      {open && <div className="border-t border-line px-3 py-3">{children}</div>}
    </div>
  );
}


/** One placement line: a label, its chips, and an add button. */
function Line({ label, empty, children, add }: { label: string; empty: string; children: ReactNode[]; add: ReactNode }) {
  return (
    <div className="flex min-w-0 flex-wrap items-center gap-1.5 px-3 py-2">
      <span className="w-24 shrink-0 text-xs font-medium text-muted">{label}</span>
      {children.length ? children : <span className="text-xs text-faint">{empty}</span>}
      {add}
    </div>
  );
}

/** A removable chip; clicking its text edits it. */
function Chip({ text, title, bad, onEdit, onRemove, removeLabel }: { text: ReactNode; title: string; bad?: boolean; onEdit?: () => void; onRemove: () => void; removeLabel: string }) {
  return (
    <span className={`inline-flex h-6 max-w-full items-center rounded-md border bg-panel-2 font-mono text-[11px] ${bad ? "border-bad text-bad" : "border-line text-fg"}`} title={title}>
      {onEdit ? (
        <button type="button" onClick={onEdit} className="min-w-0 truncate rounded-l-md px-2 hover:text-accent focus-visible:outline-2 focus-visible:outline-accent">
          {text}
        </button>
      ) : (
        <span className="min-w-0 truncate px-2">{text}</span>
      )}
      <button type="button" aria-label={removeLabel} onClick={onRemove} className="grid h-full w-5 place-items-center rounded-r-md text-faint hover:text-bad focus-visible:outline-2 focus-visible:outline-accent">
        <X className="size-3" />
      </button>
    </span>
  );
}

const AddButton = ({ label, onClick, disabled, title }: { label: string; onClick: () => void; disabled?: boolean; title?: string }) => (
  <button
    type="button"
    onClick={onClick}
    disabled={disabled}
    title={title}
    className="inline-flex h-6 items-center gap-1 rounded-md border border-dashed border-line px-2 text-[11px] text-muted hover:border-accent hover:text-accent disabled:opacity-50 focus-visible:outline-2 focus-visible:outline-accent"
  >
    <Plus className="size-3" /> {label}
  </button>
);

/** Readable form of a rule: `forge.db = postgres`, `role ≠ manager`. */
function ruleText(r: Row) {
  const lhs = r.key.endsWith(".labels.") ? `${r.key.startsWith("engine") ? "engine." : ""}${r.label || "?"}` : r.key.replace(/^node\.(platform\.)?/, "");
  return `${lhs} ${r.op === "!=" ? "≠" : "="} ${r.value || "?"}`;
}

function Constraints({ value, onChange }: { value: string[]; onChange: (v: string[]) => void }) {
  const rows = useRowIds(value.length);
  // The rule being edited: an index, "new", or none.
  const [edit, setEdit] = useState<{ at: number | "new"; row: Row } | null>(null);
  const save = () => {
    if (!edit) return;
    const c = format(edit.row);
    if (edit.at === "new") {
      rows.add();
      onChange([...value, c]);
    } else onChange(value.map((x, j) => (j === edit.at ? c : x)));
    setEdit(null);
  };
  return (
    <div>
      <Line
        label="Run on"
        empty="any active node"
        add={!edit && <AddButton label="Rule" onClick={() => setEdit({ at: "new", row: { key: "node.labels.", label: "", op: "==", value: "" } })} />}
      >
        {value.map((c, i) => (
          <Chip
            key={rows.ids[i]}
            text={ruleText(parse(c))}
            title={c}
            bad={!CONSTRAINT_RE.test(c)}
            onEdit={() => setEdit({ at: i, row: parse(c) })}
            removeLabel={`Remove rule ${c}`}
            onRemove={() => {
              rows.remove(i);
              onChange(value.filter((_, j) => j !== i));
              setEdit(null);
            }}
          />
        ))}
      </Line>
      {edit && <RuleEditor row={edit.row} isNew={edit.at === "new"} onChange={(row) => setEdit({ ...edit, row })} onSave={save} onCancel={() => setEdit(null)} />}
    </div>
  );
}

/** The one inline editor for a rule, shown while adding or editing. */
function RuleEditor({ row, isNew, onChange, onSave, onCancel }: { row: Row; isNew: boolean; onChange: (r: Row) => void; onSave: () => void; onCancel: () => void }) {
  const k = KEYS.find((x) => x.id === row.key);
  const isLabel = row.key.endsWith(".labels.");
  const ok = CONSTRAINT_RE.test(format(row)) && !!row.value && (!isLabel || !!row.label);
  const keys = (e: React.KeyboardEvent) => {
    if (e.key === "Enter") {
      e.preventDefault();
      if (ok) onSave();
    } else if (e.key === "Escape") onCancel();
  };
  return (
    <div className="flex flex-wrap items-center gap-1.5 border-t border-dashed border-line bg-panel-2/50 px-3 py-2 pl-[7.375rem]" onKeyDown={keys}>
      <div className="w-36">
        <Select aria-label="Attribute" className="w-full text-xs" value={row.key} onChange={(e) => onChange({ ...row, key: e.target.value, label: "" })}>
          {!k && <option value={row.key}>{row.key}</option>}
          {KEYS.map((x) => (
            <option key={x.id} value={x.id}>
              {x.label}
            </option>
          ))}
        </Select>
      </div>
      {isLabel && (
        <div className="w-28">
          <Input aria-label="Label name" autoFocus className="h-8 py-0 font-mono text-xs" placeholder="label" value={row.label} onChange={(e) => onChange({ ...row, label: e.target.value.trim() })} />
        </div>
      )}
      <button
        type="button"
        aria-label={row.op === "==" ? "Equals; switch to not equals" : "Not equals; switch to equals"}
        onClick={() => onChange({ ...row, op: row.op === "==" ? "!=" : "==" })}
        className="h-8 w-8 rounded-md border border-line bg-panel font-mono text-sm hover:border-accent focus-visible:outline-2 focus-visible:outline-accent"
      >
        {row.op === "==" ? "=" : "≠"}
      </button>
      <div className="w-36">
        <Input aria-label="Value" autoFocus={!isLabel} className="h-8 py-0 font-mono text-xs" placeholder={k?.hint ?? "value"} value={row.value} onChange={(e) => onChange({ ...row, value: e.target.value.trim() })} />
      </div>
      <Button type="button" variant="primary" disabled={!ok} onClick={onSave}>
        {isNew ? "Add" : "Update"}
      </Button>
      <Button type="button" variant="ghost" onClick={onCancel}>
        Cancel
      </Button>
    </div>
  );
}

function Spread({ value, onChange }: { value: string[]; onChange: (v: string[]) => void }) {
  const rows = useRowIds(value.length);
  const [draft, setDraft] = useState<string | null>(null);
  const name = (s: string) => s.replace(/^node\.labels\./, "");
  const add = () => {
    const v = `node.labels.${(draft ?? "").trim()}`;
    if (!SPREAD_RE.test(v)) return;
    rows.add();
    onChange([...value, v]);
    setDraft(null);
  };
  const full = value.length >= 4;
  return (
    <Line
      label="Spread over"
      empty="nothing; the swarm balances by load"
      add={
        draft === null ? (
          <AddButton label="Label" disabled={full} title={full ? "At most 4 spread labels" : undefined} onClick={() => setDraft("")} />
        ) : (
          <span className="inline-flex items-center gap-1">
            <div className="w-28">
              <Input
                aria-label="Spread label"
                autoFocus
                className="h-6 py-0 font-mono text-[11px]"
                placeholder="zone"
                value={draft}
                onChange={(e) => setDraft(e.target.value.trim())}
                onBlur={() => (draft ? add() : setDraft(null))}
                onKeyDown={(e) => {
                  if (e.key === "Enter") {
                    e.preventDefault();
                    add();
                  } else if (e.key === "Escape") setDraft(null);
                }}
              />
            </div>
          </span>
        )
      }
    >
      {value.map((s, i) => (
        <Chip
          key={rows.ids[i]}
          text={name(s)}
          title={s}
          bad={!SPREAD_RE.test(s)}
          removeLabel={`Remove spread label ${name(s)}`}
          onRemove={() => {
            rows.remove(i);
            onChange(value.filter((_, j) => j !== i));
          }}
        />
      ))}
    </Line>
  );
}
