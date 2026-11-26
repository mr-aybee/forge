import { Plus, ShieldAlert, Trash2 } from "lucide-react";
import { type ReactNode, useId } from "react";

import type { RestartPolicy, ServiceInput } from "../../api/types";
import { Badge, Button, IconButton, Input, Select } from "../ui";
import { RESTART_POLICIES, policyOf, sectionDomId, type SectionMeta, type SectionStatus } from "./sections";
import { StatusMark } from "./StatusMark";

/** One always-open section card at full width; its id is the rail's scroll target. */
export function SectionCard({ meta, status, messages, actions, children }: { meta: SectionMeta; status: SectionStatus; messages: string[]; actions?: ReactNode; children: ReactNode }) {
  const Icon = meta.icon;
  return (
    <section id={sectionDomId(meta.id)} aria-labelledby={`${sectionDomId(meta.id)}-title`} className="scroll-mt-4 rounded-lg border border-line bg-panel">
      <header className="card-head flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 rounded-t-lg border-b border-line px-4 py-2.5">
        <Icon className="size-4 shrink-0 text-accent" aria-hidden />
        <h3 id={`${sectionDomId(meta.id)}-title`} className="text-[13px] font-semibold">
          {meta.label}
        </h3>
        <StatusMark status={status} />
        <span className="min-w-0 flex-1 truncate text-xs text-muted" title={meta.hint}>
          {meta.hint}
        </span>
        {actions && <div className="flex shrink-0 items-center gap-1">{actions}</div>}
      </header>
      <div className="space-y-4 px-4 py-4">
        {children}
        {messages.length > 0 && (
          <ul className={`space-y-0.5 text-xs ${status === "error" ? "text-bad" : "text-warn"}`} aria-live="polite">
            {messages.map((m) => (
              <li key={m}>{m}</li>
            ))}
          </ul>
        )}
      </div>
    </section>
  );
}

/** Two-column field grid; `wide` children span both columns. */
export function Grid({ children }: { children: ReactNode }) {
  return <div className="grid gap-x-4 gap-y-3 md:grid-cols-2">{children}</div>;
}

/**
 * Label above, control, hint below; `error` replaces the hint in red and
 * marks the control invalid. `render` gets the id to put on the control.
 */
export function FormField({ label, hint, error, wide, children }: { label: string; hint?: ReactNode; error?: string; wide?: boolean; children: (id: string, invalid: boolean) => ReactNode }) {
  const id = useId();
  return (
    <div className={`min-w-0 space-y-1 ${wide ? "md:col-span-2" : ""}`}>
      <label htmlFor={id} className="block text-xs font-medium text-muted">
        {label}
      </label>
      {children(id, !!error)}
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

/** A number input whose empty value is `0` (no limit) or `null`. */
export function NumberInput({ id, value, onChange, empty = 0, invalid, ...rest }: { id: string; value: number | null; onChange: (v: number) => void; empty?: number; invalid?: boolean; min?: number; max?: number; step?: number; placeholder?: string; className?: string; label?: string }) {
  return (
    <Input
      id={id}
      aria-label={rest.label}
      type="number"
      inputMode="decimal"
      aria-invalid={invalid || undefined}
      className={`font-mono tabular-nums ${invalid ? "border-bad" : ""} ${rest.className ?? ""}`}
      value={value === null || value === empty ? "" : value}
      min={rest.min}
      max={rest.max}
      step={rest.step}
      placeholder={rest.placeholder}
      onChange={(e) => onChange(e.target.value === "" ? empty : Number(e.target.value))}
    />
  );
}

/** Dense one-value-per-row list (ports, volumes, hosts) with inline validation. */
export function ListEditor({ label, values, onChange, placeholder, validate, addLabel = "Add" }: { label: string; values: string[]; onChange: (v: string[]) => void; placeholder: string; validate?: (v: string) => boolean; addLabel?: string }) {
  return (
    <div className="space-y-1.5">
      {values.length === 0 ? (
        <p className="rounded-md border border-dashed border-line px-3 py-2 text-xs text-faint">None yet.</p>
      ) : (
        <ul className="divide-y divide-line rounded-md border border-line">
          {values.map((v, i) => {
            const bad = !!v && !!validate && !validate(v);
            return (
              // Index keys: rows have no identity of their own, and order is the value.
              <li key={i} className="flex items-center gap-2 px-2 py-1">
                <span className="w-5 shrink-0 text-right font-mono text-[11px] text-faint tabular-nums">{i + 1}</span>
                <Input
                  aria-label={`${label} ${i + 1}`}
                  aria-invalid={bad || undefined}
                  className={`h-7 py-0 font-mono text-xs ${bad ? "border-bad" : ""}`}
                  value={v}
                  placeholder={placeholder}
                  onChange={(e) => onChange(values.map((x, j) => (j === i ? e.target.value.trim() : x)))}
                />
                <IconButton type="button" label={`Remove ${label.toLowerCase()} ${i + 1}`} className="size-7 hover:text-bad" onClick={() => onChange(values.filter((_, j) => j !== i))}>
                  <Trash2 className="size-3.5" />
                </IconButton>
              </li>
            );
          })}
        </ul>
      )}
      <Button type="button" variant="ghost" onClick={() => onChange([...values, ""])}>
        <Plus className="size-4" /> {addLabel}
      </Button>
    </div>
  );
}

/** A checkbox with its label and an optional hint. */
export function Check({ checked, onChange, label, hint }: { checked: boolean; onChange: (v: boolean) => void; label: string; hint?: string }) {
  return (
    <label className="flex items-start gap-2">
      <input type="checkbox" className="mt-0.5" checked={checked} onChange={(e) => onChange(e.target.checked)} />
      <span className="min-w-0">
        <span className="text-[13px]">{label}</span>
        {hint && <span className="block text-xs text-faint">{hint}</span>}
      </span>
    </label>
  );
}

/** Restart policy select, plus the attempt limit when it is `on-failure`. */
export function RestartFields({ value, set, systemd = false }: { value: ServiceInput; set: (p: Partial<ServiceInput>) => void; systemd?: boolean }) {
  const policy = policyOf(value);
  const attemptsBad = policy === "on-failure" && (!Number.isInteger(value.restart_max_attempts) || value.restart_max_attempts < 0 || value.restart_max_attempts > 1000);
  return (
    <>
      <FormField label="Restart policy" hint={systemd ? "systemd Restart=: always, on-failure or no. Unless stopped behaves like always." : "Unless stopped is Forge's default: restart unless an operator stopped it."}>
        {(id) => (
          <Select id={id} value={policy} onChange={(e) => {
            const p = e.target.value as RestartPolicy;
            set({ restart_policy: p, restart: p !== "no" });
          }}>
            {RESTART_POLICIES.map((r) => (
              <option key={r.id} value={r.id}>
                {r.label}
              </option>
            ))}
          </Select>
        )}
      </FormField>
      {policy === "on-failure" && (
        <FormField label="Max restart attempts" error={attemptsBad ? "0 (unlimited) to 1000." : undefined} hint="After a non-zero exit. Empty or 0: no limit.">
          {(id, invalid) => <NumberInput id={id} className="w-36" min={0} max={1000} value={value.restart_max_attempts} invalid={invalid} placeholder="unlimited" onChange={(n) => set({ restart_max_attempts: Math.round(n) || 0 })} />}
        </FormField>
      )}
    </>
  );
}

/** Small marker for settings that need stack.privileged to deploy. */
export function PrivilegedHint() {
  return (
    <span title="Deploying needs the stack.privileged permission (administrators) and an MFA-verified sign-in. You can still save." className="inline-flex">
      <Badge tone="warn">
        <ShieldAlert className="mr-1 size-3" aria-hidden /> needs stack.privileged
      </Badge>
    </span>
  );
}

/** A labelled block inside a section: small heading with optional badge, then content. */
export function SubBlock({ title, badge, hint, children }: { title: string; badge?: ReactNode; hint?: ReactNode; children: ReactNode }) {
  return (
    <div className="min-w-0 space-y-1 md:col-span-2">
      <div className="flex min-w-0 flex-wrap items-center gap-1.5">
        <p className="text-xs font-medium text-muted">{title}</p>
        {badge}
      </div>
      {children}
      {hint && <p className="text-xs text-faint">{hint}</p>}
    </div>
  );
}

/** Dense rows of several inputs each (ulimits, tmpfs, devices, sysctls). */
export function RowsEditor<T>({ label, rows, onChange, blank, addLabel, render, max }: { label: string; rows: T[]; onChange: (v: T[]) => void; blank: () => T; addLabel: string; render: (row: T, set: (r: T) => void, i: number) => ReactNode; max?: number }) {
  return (
    <div className="space-y-1.5">
      {rows.length === 0 ? (
        <p className="rounded-md border border-dashed border-line px-3 py-2 text-xs text-faint">None yet.</p>
      ) : (
        <ul className="divide-y divide-line rounded-md border border-line">
          {rows.map((r, i) => (
            // Index keys: rows have no identity of their own.
            <li key={i} className="flex min-w-0 items-center gap-2 px-2 py-1">
              <span className="w-5 shrink-0 text-right font-mono text-[11px] text-faint tabular-nums">{i + 1}</span>
              <div className="flex min-w-0 flex-1 flex-wrap items-center gap-1.5 sm:flex-nowrap">{render(r, (n) => onChange(rows.map((x, j) => (j === i ? n : x))), i)}</div>
              <IconButton type="button" label={`Remove ${label.toLowerCase()} ${i + 1}`} className="size-7 hover:text-bad" onClick={() => onChange(rows.filter((_, j) => j !== i))}>
                <Trash2 className="size-3.5" />
              </IconButton>
            </li>
          ))}
        </ul>
      )}
      <Button type="button" variant="ghost" disabled={max !== undefined && rows.length >= max} title={max !== undefined && rows.length >= max ? `At most ${max}` : undefined} onClick={() => onChange([...rows, blank()])}>
        <Plus className="size-4" /> {addLabel}
      </Button>
    </div>
  );
}
