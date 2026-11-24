import { Plus, Trash2 } from "lucide-react";
import { type ReactNode, useId } from "react";

import { Button, IconButton, Input } from "../ui";
import { sectionDomId, type SectionMeta, type SectionStatus } from "./sections";
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
export function NumberInput({ id, value, onChange, empty = 0, invalid, ...rest }: { id: string; value: number | null; onChange: (v: number) => void; empty?: number; invalid?: boolean; min?: number; max?: number; step?: number; placeholder?: string; className?: string }) {
  return (
    <Input
      id={id}
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
