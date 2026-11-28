import { useQuery } from "@tanstack/react-query";
import { Activity, Bell, Check, Crosshair, FilePlus2, LifeBuoy, type LucideIcon, Loader2, Power, Sparkles, Tag, X } from "lucide-react";
import { type ReactNode, useMemo, useState } from "react";

import {
  type AlertRule,
  type AlertTemplate,
  type Condition,
  type Mode,
  type Place,
  type RuleInput,
  type RuleScopeKind,
  type Severity,
  subjectInfo,
  useAlertSubjects,
  useAlertTemplates,
  useSaveRule,
} from "../../api/alerts";
import { get } from "../../api/client";
import { useServices, useInstallations, useServers } from "../../api/hooks";
import { nodeName } from "../../api/types";
import { errorText, useToast } from "../../app/toast";
import { Button, Field, Input, Modal, Select, Skeleton, Switch, short } from "../ui";
import type { Certificate } from "../ingress/types";
import { FormError } from "./parts";

type ModeKind = "at_once" | "sustained" | "window_average" | "repeated" | "any_in_window";
type Cmp = "gt" | "ge" | "lt" | "le" | "eq" | "ne";
type DUnit = "seconds" | "minutes" | "hours";
/** A duration as the user types it: an amount and a unit. */
interface Dur {
  n: string;
  unit: DUnit;
}

const UNIT_MS: Record<DUnit, number> = { seconds: 1000, minutes: 60_000, hours: 3_600_000 };

/** 20 000 -> 20 seconds, 300 000 -> 5 minutes, 7 200 000 -> 2 hours: the largest unit that gives a whole number. */
function toDur(ms: number): Dur {
  const v = Number.isFinite(ms) && ms > 0 ? ms : 0;
  if (v === 0) return { n: "0", unit: "minutes" };
  if (v % UNIT_MS.hours === 0) return { n: String(v / UNIT_MS.hours), unit: "hours" };
  if (v % UNIT_MS.minutes === 0) return { n: String(v / UNIT_MS.minutes), unit: "minutes" };
  // Whole seconds, or at most three decimals so no float noise shows.
  return { n: String(Math.round(v) % 1000 === 0 ? v / 1000 : Math.round(v) / 1000), unit: "seconds" };
}

/** Milliseconds, or NaN when the amount is not a number of 0 or more. */
function durMs(d: Dur): number {
  const n = d.n.trim() === "" ? NaN : Number(d.n);
  return n >= 0 ? Math.round(n * UNIT_MS[d.unit]) : NaN;
}

/** "20 seconds", "1 minute", "2 hours". */
function durText(d: Dur): string {
  const ms = durMs(d);
  if (!Number.isFinite(ms)) return "…";
  const x = toDur(ms);
  const n = Number(x.n);
  return `${x.n} ${n === 1 ? x.unit.slice(0, -1) : x.unit}`;
}

/** What a dialog opened from a node, service, database or certificate starts with. */
export interface RulePrefill {
  /** Offer only this place's templates. */
  place?: Place;
  /** The template chosen first. */
  templateId?: string;
  /** The resources each scope kind targets (a database: its installation and its node). */
  targets?: Partial<Record<RuleScopeKind, string[]>>;
  /** The health check that fills `{check}`. */
  check?: string;
}

/** The whole form: one object, so a new template is one reset. */
interface Form {
  name: string;
  description: string;
  severity: Severity;
  subject: string;
  enabled: boolean;
  kind: "state" | "threshold";
  /** `state_not_in` instead of `state_in`. */
  negate: boolean;
  states: string[];
  cmp: Cmp;
  threshold: string;
  mode: ModeKind;
  window: Dur;
  count: string;
  spacing: Dur;
  coverage: number;
  recover: string[];
  recoverAt: string;
  hold: Dur;
  scopeKind: RuleScopeKind;
  scopeIds: string[];
}

const SCOPES: { id: RuleScopeKind; label: string; one: string; all: string }[] = [
  { id: "servers", label: "Nodes", one: "a node", all: "All nodes" },
  { id: "services", label: "Services", one: "a service", all: "All services" },
  { id: "installations", label: "Databases and packages", one: "a database or package", all: "All databases and packages" },
  { id: "certificates", label: "Certificates", one: "a certificate", all: "All certificates" },
];

const FAMILY: Record<RuleScopeKind, string> = { servers: "Node", services: "Service", installations: "Database", certificates: "Certificate" };

const CMP_TEXT: Record<Cmp, string> = { gt: ">", ge: "≥", lt: "<", le: "≤", eq: "=", ne: "≠" };
const CMP_WORDS: Record<Cmp, string> = { gt: "above", ge: "at or above", lt: "below", le: "at or below", eq: "equal to", ne: "not equal to" };

/** The suffix a value of this unit carries. */
function unitSuffix(unit: string | undefined): string | undefined {
  switch (unit) {
    case "percent":
      return "%";
    case "ratio":
      return "ratio";
    case "seconds":
      return "s";
    case "days":
      return "days";
    default:
      return unit || undefined;
  }
}

function modeFields(m: Mode): Pick<Form, "mode" | "window" | "count" | "spacing" | "coverage"> {
  const base = { window: toDur(300_000), count: "3", spacing: toDur(0), coverage: 80 };
  switch (m.mode) {
    case "sustained":
      return { ...base, mode: "sustained", window: toDur(m.for_ms) };
    case "window_average":
      return { ...base, mode: "window_average", window: toDur(m.window_ms), coverage: m.min_coverage_pct ?? 80 };
    case "repeated":
      return { ...base, mode: "repeated", window: toDur(m.window_ms), count: String(m.count), spacing: toDur(m.min_spacing_ms ?? 0) };
    case "any_in_window":
      return { ...base, mode: "any_in_window", window: toDur(m.window_ms) };
    default:
      return { ...base, mode: "at_once" };
  }
}

function specFields(spec: RuleInput["spec"]): Omit<Form, "name" | "description" | "severity" | "subject" | "enabled" | "scopeKind" | "scopeIds"> {
  const t = spec.trigger;
  const r = spec.recovery?.condition;
  return {
    kind: t.kind === "threshold" ? "threshold" : "state",
    negate: t.kind === "state_not_in",
    states: t.kind === "threshold" ? [] : (t.states ?? []),
    cmp: t.kind === "threshold" ? t.cmp : "gt",
    threshold: t.kind === "threshold" ? String(t.value) : "85",
    ...modeFields(spec.mode),
    recover: r && r.kind !== "threshold" ? (r.states ?? []) : [],
    recoverAt: r?.kind === "threshold" ? String(r.value) : "",
    hold: toDur(spec.recovery?.hold_ms ?? 0),
  };
}

function fromRule(rule: AlertRule): Form {
  return {
    name: rule.name,
    description: rule.description,
    severity: rule.severity,
    subject: rule.subject,
    enabled: rule.enabled,
    ...specFields(rule.spec),
    scopeKind: rule.scope.kind,
    scopeIds: rule.scope.ids ?? [],
  };
}

function fromTemplate(t: AlertTemplate, prefill: RulePrefill | undefined): Form {
  const check = prefill?.check;
  return {
    name: check ? `${t.name}: ${check}` : t.name,
    description: t.description,
    severity: t.severity,
    subject: check ? t.subject.replace("{check}", check) : t.subject,
    enabled: true,
    ...specFields(t.spec),
    scopeKind: t.scope,
    scopeIds: prefill?.targets?.[t.scope] ?? [],
  };
}

function blank(prefill: RulePrefill | undefined): Form {
  const [scopeKind, ids] = (Object.entries(prefill?.targets ?? {})[0] ?? ["servers", []]) as [RuleScopeKind, string[]];
  return {
    name: "",
    description: "",
    severity: "warning",
    subject: scopeKind === "servers" ? "node.agent.status" : "",
    enabled: true,
    kind: "state",
    negate: false,
    states: ["offline"],
    cmp: "gt",
    threshold: "85",
    mode: "sustained",
    window: toDur(120_000),
    count: "3",
    spacing: toDur(0),
    coverage: 80,
    recover: [],
    recoverAt: "",
    hold: toDur(0),
    scopeKind,
    scopeIds: ids,
  };
}

/** The rule a form describes, or why it can't be saved. */
function toInput(f: Form): RuleInput | string {
  const win = durMs(f.window);
  const hold = durMs(f.hold);
  const spacing = durMs(f.spacing);
  const count = Number(f.count);
  if (!f.name.trim()) return "Name the rule.";
  if (!f.subject.trim()) return "Pick a subject.";
  if (f.subject.includes("{check}")) return "Replace {check} in the subject with the health check's name.";
  if (!Number.isFinite(hold)) return "The hold is a duration of 0 or more.";
  let trigger: Condition;
  let recovery: Condition | null = null;
  if (f.kind === "state") {
    if (f.states.length === 0) return "Choose at least one state.";
    trigger = { kind: f.negate ? "state_not_in" : "state_in", states: f.states };
    if (f.recover.length) recovery = { kind: "state_in", states: f.recover };
  } else {
    const t = Number(f.threshold);
    if (!f.threshold.trim() || !Number.isFinite(t)) return "The threshold is a number.";
    trigger = { kind: "threshold", cmp: f.cmp, value: t };
    if (f.recoverAt.trim()) {
      const r = Number(f.recoverAt);
      if (!Number.isFinite(r)) return "The recovery threshold is a number.";
      recovery = { kind: "threshold", cmp: f.cmp === "gt" || f.cmp === "ge" ? "lt" : "gt", value: r };
    }
  }
  if (f.mode !== "at_once" && !(win > 0)) return "The duration is more than 0.";
  if (f.mode === "window_average" && f.kind !== "threshold") return "An average needs a number condition.";
  if (f.mode === "repeated" && !(Number.isInteger(count) && count >= 1 && count <= 1000)) return "The count is a whole number from 1 to 1000.";
  if (f.mode === "repeated" && !Number.isFinite(spacing)) return "The spacing is a duration of 0 or more.";
  const mode: Mode =
    f.mode === "sustained"
      ? { mode: "sustained", for_ms: win }
      : f.mode === "window_average"
        ? { mode: "window_average", window_ms: win, min_coverage_pct: f.coverage }
        : f.mode === "repeated"
          ? { mode: "repeated", count, window_ms: win, min_spacing_ms: spacing }
          : f.mode === "any_in_window"
            ? { mode: "any_in_window", window_ms: win }
            : { mode: "immediate" };
  return {
    name: f.name.trim(),
    description: f.description,
    enabled: f.enabled,
    severity: f.severity,
    subject: f.subject.trim(),
    scope: { kind: f.scopeKind, ids: f.scopeIds },
    spec: { trigger, mode, recovery: { condition: recovery, hold_ms: hold } },
  };
}

/** "Fires when a node's node.agent.status is offline for 20 seconds; recovers when online." */
function summary(f: Form, unit: string | undefined): string {
  const scope = SCOPES.find((s) => s.id === f.scopeKind);
  const n = f.scopeIds.length;
  const plural = scope?.label.toLowerCase() ?? "resources";
  const singular = (scope?.one ?? "a resource").replace(/^an? /, "");
  const who = n === 0 ? `any ${singular}` : n === 1 ? `the chosen ${singular}` : `any of ${n} chosen ${plural}`;
  const sfx = unitSuffix(unit);
  const val = (v: string) => (sfx === "%" ? `${v} %` : sfx ? `${v} ${sfx}` : v);
  const what =
    f.kind === "state"
      ? `is ${f.negate ? "not " : ""}${f.states.length ? f.states.join(" or ") : "…"}`
      : f.mode === "window_average"
        ? `averages ${CMP_WORDS[f.cmp]} ${val(f.threshold || "…")}`
        : `is ${CMP_WORDS[f.cmp]} ${val(f.threshold || "…")}`;
  const when =
    f.mode === "at_once"
      ? "at once"
      : f.mode === "sustained"
        ? `for ${durText(f.window)}`
        : f.mode === "window_average"
          ? `over ${durText(f.window)}`
          : f.mode === "repeated"
            ? `${f.count || "…"} times within ${durText(f.window)}`
            : `at any point within ${durText(f.window)}`;
  const rec = f.kind === "state" ? (f.recover.length ? `when ${f.recover.join(" or ")}` : "when the condition no longer holds") : f.recoverAt.trim() ? `past ${val(f.recoverAt)}` : "when the condition no longer holds";
  const hold = durMs(f.hold) > 0 ? ` for ${durText(f.hold)}` : "";
  return `Fires when ${f.subject || "the subject"} on ${who} ${what} ${when}; recovers ${rec}${hold}.`;
}

/**
 * Create or edit an alert rule (D-118). A new rule can start from a template;
 * picking another template remounts the form, so its values are the reset.
 */
export function RuleDialog({ rule, prefill, onClose }: { rule: AlertRule | null; prefill?: RulePrefill; onClose: () => void }) {
  const templates = useAlertTemplates(prefill?.place);
  const [templateId, setTemplateId] = useState(prefill?.templateId ?? "");
  const template = templates.data?.find((t) => t.id === templateId);
  const waiting = !rule && !!templateId && templates.isLoading;
  const picker = rule ? undefined : <TemplatePicker templates={templates.data} loading={templates.isLoading} error={templates.error} value={templateId} onChange={setTemplateId} />;
  return (
    <Modal
      open
      onOpenChange={(o) => !o && onClose()}
      width={920}
      fill
      title={rule ? "Edit rule" : "New alert rule"}
      description={rule ? "Changing the condition, subject or scope starts a new revision; firing alarms of the old one end." : "Start from a template or describe the condition yourself."}
    >
      {waiting ? (
        <div className="space-y-4 overflow-y-auto">
          {picker}
          <Skeleton rows={4} />
        </div>
      ) : (
        <RuleForm
          key={rule ? rule.id : `${templateId}:${template ? "t" : "-"}`}
          ruleId={rule?.id}
          picker={picker}
          initial={rule ? fromRule(rule) : template ? fromTemplate(template, prefill) : blank(prefill)}
          onClose={onClose}
        />
      )}
    </Modal>
  );
}

type Tone = "accent" | "violet" | "teal" | "ok" | "orange" | "warn" | "bad" | "sky";

/** Token classes per tone (spelled out so Tailwind sees them). */
const TONE: Record<Tone, { chip: string; on: string; dot: string }> = {
  accent: { chip: "bg-accent-soft text-accent", on: "border-accent bg-accent-soft text-accent", dot: "bg-accent" },
  violet: { chip: "bg-violet-soft text-violet", on: "border-violet bg-violet-soft text-violet", dot: "bg-violet" },
  teal: { chip: "bg-teal-soft text-teal", on: "border-teal bg-teal-soft text-teal", dot: "bg-teal" },
  ok: { chip: "bg-ok-soft text-ok", on: "border-ok bg-ok-soft text-ok", dot: "bg-ok" },
  orange: { chip: "bg-orange-soft text-orange", on: "border-orange bg-orange-soft text-orange", dot: "bg-orange" },
  warn: { chip: "bg-warn-soft text-warn", on: "border-warn bg-warn-soft text-warn", dot: "bg-warn" },
  bad: { chip: "bg-bad-soft text-bad", on: "border-bad bg-bad-soft text-bad", dot: "bg-bad" },
  sky: { chip: "bg-sky-soft text-sky", on: "border-sky bg-sky-soft text-sky", dot: "bg-sky" },
};

const SEVERITY: { id: Severity; tone: Tone }[] = [
  { id: "info", tone: "sky" },
  { id: "warning", tone: "warn" },
  { id: "major", tone: "orange" },
  { id: "critical", tone: "bad" },
];
const sevTone = (s: Severity): Tone => SEVERITY.find((x) => x.id === s)?.tone ?? "accent";

/** One numbered step of the form: a card with a gradient head, a tone icon chip and a done mark. */
function Step({ n, title, hint, icon: Icon, tone, done, children }: { n: number; title: string; hint?: string; icon: LucideIcon; tone: Tone; done: boolean; children: ReactNode }) {
  return (
    <section aria-labelledby={`rule-step-${n}`} className="overflow-hidden rounded-lg border border-line">
      <header className="card-head flex min-w-0 items-center gap-2 border-b border-line px-3 py-1.5">
        <span className="grid size-5 shrink-0 place-items-center rounded-full border border-line-strong bg-panel font-mono text-[11px] tabular-nums text-muted">{n}</span>
        <span className={`grid size-6 shrink-0 place-items-center rounded-md ${TONE[tone].chip}`}>
          <Icon aria-hidden className="size-3.5" />
        </span>
        <h3 id={`rule-step-${n}`} className="text-[13px] font-semibold">
          {title}
        </h3>
        {hint && <span className="min-w-0 truncate text-xs text-faint" title={hint}>{hint}</span>}
        <span className={`ml-auto inline-flex shrink-0 items-center gap-1 rounded-full px-1.5 py-0.5 text-[11px] ${done ? "bg-ok-soft text-ok" : "bg-panel-2 text-faint"}`}>
          {done ? <Check aria-hidden className="size-3" /> : <span aria-hidden className="size-1.5 rounded-full bg-faint" />}
          {done ? "Done" : "To do"}
        </span>
      </header>
      <div className="space-y-3 p-3">{children}</div>
    </section>
  );
}

/** Templates as small clickable cards, "Blank rule" first. */
function TemplatePicker({ templates, loading, error, value, onChange }: { templates: AlertTemplate[] | undefined; loading: boolean; error: unknown; value: string; onChange: (id: string) => void }) {
  const list = Array.isArray(templates) ? templates : [];
  const card = (id: string, name: string, desc: string, Icon: LucideIcon, tone: Tone) => {
    const on = value === id;
    return (
      <button
        key={id || "blank"}
        type="button"
        aria-pressed={on}
        onClick={() => onChange(id)}
        title={desc}
        className={`flex min-w-0 items-start gap-2 rounded-md border px-2 py-1.5 text-left transition focus-visible:ring-2 focus-visible:ring-accent focus-visible:outline-none ${on ? TONE.accent.on : "border-line hover:border-line-strong hover:bg-panel-2"}`}
      >
        <span className={`mt-0.5 grid size-5 shrink-0 place-items-center rounded ${TONE[tone].chip}`}>
          <Icon aria-hidden className="size-3" />
        </span>
        <span className="min-w-0">
          <span className={`block truncate text-xs font-medium ${on ? "text-accent" : "text-fg"}`}>{name}</span>
          <span className="block truncate text-[11px] text-muted">{desc || "—"}</span>
        </span>
      </button>
    );
  };
  return (
    <div className="space-y-1.5">
      <span className="block text-xs font-medium text-muted">Start from</span>
      <div role="group" aria-label="Template" className="grid gap-1.5 sm:grid-cols-2 lg:grid-cols-3">
        {card("", "Blank rule", "Describe the condition yourself.", FilePlus2, "accent")}
        {list.map((t) => card(t.id, t.name, t.description, Bell, sevTone(t.severity)))}
      </div>
      {loading && <Skeleton rows={1} />}
      {!!error && <FormError>Templates could not load: {errorText(error)}. Start from a blank rule.</FormError>}
    </div>
  );
}

/** An amount with a seconds / minutes / hours unit. */
function DurationInput({ label, value, onChange }: { label: string; value: Dur; onChange: (d: Dur) => void }) {
  return (
    <div className="flex gap-1.5">
      <Input aria-label={label} inputMode="decimal" className="w-20" value={value.n} onChange={(e) => onChange({ ...value, n: e.target.value })} />
      <div className="w-28">
        <Select aria-label={`${label} unit`} value={value.unit} onChange={(e) => onChange({ ...value, unit: e.target.value as DUnit })}>
          <option value="seconds">seconds</option>
          <option value="minutes">minutes</option>
          <option value="hours">hours</option>
        </Select>
      </div>
    </div>
  );
}

/** Toggle chips for the states a subject reports; free text when the catalogue lists none. */
function StateChips({ label, options, value, onChange }: { label: string; options: string[] | undefined; value: string[]; onChange: (v: string[]) => void }) {
  const [text, setText] = useState(value.join(", "));
  if (!options?.length)
    return (
      <Input
        aria-label={label}
        placeholder="Comma-separated states"
        value={text}
        onChange={(e) => {
          setText(e.target.value);
          onChange(
            e.target.value
              .split(",")
              .map((x) => x.trim())
              .filter(Boolean),
          );
        }}
      />
    );
  const all = [...options, ...value.filter((v) => !options.includes(v))];
  return (
    <div role="group" aria-label={label} className="flex flex-wrap gap-1.5">
      {all.map((s) => {
        const on = value.includes(s);
        return (
          <button
            key={s}
            type="button"
            aria-pressed={on}
            onClick={() => onChange(on ? value.filter((x) => x !== s) : [...value, s])}
            className={`inline-flex items-center gap-1 rounded-md border px-2 py-0.5 font-mono text-[11px] transition ${on ? "border-accent bg-accent-soft text-accent" : "border-line text-muted hover:text-fg"}`}
          >
            {on && <Check aria-hidden className="size-3" />}
            {s}
          </button>
        );
      })}
    </div>
  );
}

const CUSTOM = "\u0000custom";

function RuleForm({ ruleId, initial, picker, onClose }: { ruleId?: string; initial: Form; picker?: ReactNode; onClose: () => void }) {
  const save = useSaveRule();
  const toast = useToast();
  const subjects = useAlertSubjects();
  const [f, setF] = useState<Form>(initial);
  const set = (p: Partial<Form>) => setF((x) => ({ ...x, ...p }));
  const catalogue = subjects.data?.subjects;
  const info = subjectInfo(catalogue, f.subject);
  const input = useMemo(() => toInput(f), [f]);
  const fixed = useMemo(() => {
    const list = Array.isArray(catalogue) ? catalogue : [];
    const seen = Array.isArray(subjects.data?.seen_health) ? subjects.data.seen_health : [];
    return [...list.filter((x) => !x.pattern).map((x) => x.subject), ...seen.filter((s) => !list.some((x) => x.subject === s))];
  }, [catalogue, subjects.data?.seen_health]);
  const patterns = (catalogue ?? []).filter((x) => x.pattern);
  // A subject typed by hand: a pattern (health.<purpose>.<check>) or one the catalogue does not list.
  // Derived once the catalogue is in, so a listed subject never shows as "custom" while it loads.
  const [manual, setCustom] = useState(false);
  const custom = manual || (!!catalogue && !!f.subject && !fixed.includes(f.subject) && !catalogue.some((x) => x.subject === f.subject));
  const grouped = useMemo(() => {
    const out = new Map<string, string[]>();
    for (const s of fixed) {
      const i = subjectInfo(catalogue, s);
      const fam = i?.scopes?.[0] ? FAMILY[i.scopes[0]] : s.startsWith("health.") ? "Service" : "Other";
      out.set(fam, [...(out.get(fam) ?? []), s]);
    }
    return [...out.entries()].sort(([a], [b]) => ["Node", "Service", "Database", "Certificate", "Other"].indexOf(a) - ["Node", "Service", "Database", "Certificate", "Other"].indexOf(b));
  }, [fixed, catalogue]);
  const pickSubject = (subject: string) => {
    const i = subjectInfo(catalogue, subject);
    const kind: Form["kind"] | undefined = i ? (i.kind === "number" ? "threshold" : "state") : undefined;
    set({
      subject,
      ...(kind ? { kind } : {}),
      ...(kind && kind !== f.kind ? { states: [], recover: [] } : {}),
      ...(i && !i.scopes.includes(f.scopeKind) && i.scopes[0] ? { scopeKind: i.scopes[0], scopeIds: [] } : {}),
    });
  };
  const sfx = unitSuffix(info?.unit);
  const err = typeof input === "string" ? input : undefined;
  const trig = f.kind === "state" ? f.states.length > 0 : !!f.threshold.trim() && Number.isFinite(Number(f.threshold));
  const done = {
    what: !!f.name.trim(),
    cond: !!f.subject.trim() && !f.subject.includes("{check}") && trig && (f.mode === "at_once" || durMs(f.window) > 0),
    rec: Number.isFinite(durMs(f.hold)) && (f.kind === "state" || !f.recoverAt.trim() || Number.isFinite(Number(f.recoverAt))),
  };

  return (
    <form
      className="-mx-5 -mb-5 flex min-h-0 flex-1 flex-col"
      onSubmit={(e) => {
        e.preventDefault();
        if (typeof input === "string") return;
        save.mutate(
          { id: ruleId, rule: input },
          {
            onSuccess: () => {
              toast(ruleId ? "Rule saved" : "Rule created");
              onClose();
            },
          },
        );
      }}
    >
      <div className="min-h-0 flex-1 space-y-3 overflow-y-auto px-5 pb-4">
        <Step n={1} title="What" hint="Name it and say how loud it is" icon={Tag} tone="violet" done={done.what}>
          {picker}
          <div className="grid gap-3 sm:grid-cols-[1fr_auto]">
            <Field label="Name">
              <Input value={f.name} maxLength={120} onChange={(e) => set({ name: e.target.value })} autoFocus />
            </Field>
            <div className="space-y-1">
              <span id="rule-sev" className="block text-xs font-medium text-muted">
                Severity
              </span>
              <div role="radiogroup" aria-labelledby="rule-sev" className="flex rounded-md border border-line p-0.5">
                {SEVERITY.map((s) => {
                  const on = f.severity === s.id;
                  return (
                    <button
                      key={s.id}
                      type="button"
                      role="radio"
                      aria-checked={on}
                      onClick={() => set({ severity: s.id })}
                      className={`inline-flex items-center gap-1 rounded border px-2 py-1 text-xs capitalize transition focus-visible:ring-2 focus-visible:ring-accent focus-visible:outline-none ${on ? `${TONE[s.tone].on} font-medium` : "border-transparent text-muted hover:text-fg"}`}
                    >
                      <span aria-hidden className={`size-1.5 rounded-full ${TONE[s.tone].dot}`} />
                      {s.id}
                    </button>
                  );
                })}
              </div>
            </div>
          </div>
        </Step>

        <Step n={2} title="Condition" hint="What to watch and when it breaches" icon={Activity} tone="accent" done={done.cond}>
          <div className="grid gap-3 sm:grid-cols-2">
            <Field
              label="Subject"
              hint={
                subjects.error ? (
                  <span className="text-bad">Subjects could not load: {errorText(subjects.error)}. Type the subject by hand.</span>
                ) : info ? (
                  <>
                    {info.description}
                    {info.unit ? ` Unit: ${info.unit}.` : ""}
                  </>
                ) : f.subject ? (
                  "Not in the catalogue; the server refuses unknown subjects."
                ) : (
                  "What the rule watches."
                )
              }
            >
              <Select
                aria-label="Subject"
                value={custom || subjects.isError ? CUSTOM : f.subject}
                disabled={subjects.isLoading}
                onChange={(e) => {
                  if (e.target.value === CUSTOM) {
                    setCustom(true);
                    if (!f.subject.startsWith("health.")) set({ subject: patterns[0]?.subject ?? "health." });
                  } else {
                    setCustom(false);
                    pickSubject(e.target.value);
                  }
                }}
              >
                <option value="">{subjects.isLoading ? "Loading subjects…" : "Choose a subject"}</option>
                {grouped.map(([fam, list]) => (
                  <optgroup key={fam} label={fam}>
                    {list.map((s) => {
                      const i = subjectInfo(catalogue, s);
                      return (
                        <option key={s} value={s}>
                          {i?.description ? `${s} · ${i.description}` : s}
                        </option>
                      );
                    })}
                  </optgroup>
                ))}
                <optgroup label="Custom">
                  <option value={CUSTOM}>{patterns.length ? `Health check: ${patterns.map((p) => p.subject).join(", ")}` : "Type a subject"}</option>
                </optgroup>
              </Select>
              {(custom || subjects.isError) && (
                <Input
                  aria-label="Custom subject"
                  className="mt-1.5 font-mono text-xs"
                  placeholder="health.<purpose>.<check>"
                  value={f.subject}
                  onChange={(e) => pickSubject(e.target.value.toLowerCase())}
                />
              )}
            </Field>
            <div className="space-y-1">
              <span className="block text-xs font-medium text-muted">Trigger</span>
              {f.kind === "state" ? (
                <div className="space-y-1.5">
                  <div className="w-32">
                    <Select aria-label="State comparison" value={f.negate ? "not" : "is"} onChange={(e) => set({ negate: e.target.value === "not" })}>
                      <option value="is">is one of</option>
                      <option value="not">is not one of</option>
                    </Select>
                  </div>
                  <StateChips label="Trigger states" options={info?.states} value={f.states} onChange={(states) => set({ states })} />
                </div>
              ) : (
                <div className="flex items-center gap-1.5">
                  <div className="w-20">
                    <Select aria-label="Compare" value={f.cmp} onChange={(e) => set({ cmp: e.target.value as Cmp })}>
                      {(Object.keys(CMP_TEXT) as Cmp[]).map((c) => (
                        <option key={c} value={c}>
                          {CMP_TEXT[c]}
                        </option>
                      ))}
                    </Select>
                  </div>
                  <Input aria-label="Threshold" inputMode="decimal" className="w-24 tabular-nums" value={f.threshold} onChange={(e) => set({ threshold: e.target.value })} />
                  {sfx && <span className="text-xs text-muted">{sfx}</span>}
                </div>
              )}
              {!info && (
                <div className="w-32 pt-1">
                  <Select aria-label="Condition kind" value={f.kind} onChange={(e) => set({ kind: e.target.value as Form["kind"] })}>
                    <option value="state">State</option>
                    <option value="threshold">Number</option>
                  </Select>
                </div>
              )}
            </div>
          </div>
          <div className="grid gap-3 sm:grid-cols-2">
            <Field label="When">
              <Select value={f.mode} onChange={(e) => set({ mode: e.target.value as ModeKind })}>
                <option value="at_once">At once</option>
                <option value="sustained">For a duration</option>
                <option value="window_average" disabled={f.kind !== "threshold"}>
                  Average over a window
                </option>
                <option value="repeated">Repeatedly in a window</option>
                <option value="any_in_window">Any spike in a window</option>
              </Select>
            </Field>
            {f.mode !== "at_once" && (
              <div className="space-y-1">
                <span className="block text-xs font-medium text-muted">{f.mode === "sustained" ? "Duration" : "Window"}</span>
                <div className="flex flex-wrap items-center gap-1.5">
                  <DurationInput label={f.mode === "sustained" ? "Duration" : "Window"} value={f.window} onChange={(window) => set({ window })} />
                  {f.mode === "repeated" && (
                    <>
                      <Input aria-label="Times" inputMode="numeric" className="w-16 tabular-nums" value={f.count} onChange={(e) => set({ count: e.target.value })} />
                      <span className="text-xs text-muted">times</span>
                    </>
                  )}
                </div>
                <span className="block text-xs text-faint">
                  {f.mode === "sustained"
                    ? "Every sample for this long."
                    : f.mode === "window_average"
                      ? `Time-weighted; needs ${f.coverage} % coverage.`
                      : f.mode === "repeated"
                        ? "Separate breaches counted in the window."
                        : "Clears once the spike leaves the window."}
                </span>
              </div>
            )}
          </div>
          {f.mode === "repeated" && (
            <Field label="Minimum spacing" hint="Breaches closer together than this count once. 0: no spacing.">
              <DurationInput label="Minimum spacing" value={f.spacing} onChange={(spacing) => set({ spacing })} />
            </Field>
          )}
        </Step>

        <Step n={3} title="Watches" hint="Which resources the rule covers" icon={Crosshair} tone="teal" done>
          <ScopePicker kind={f.scopeKind} ids={f.scopeIds} allowed={info?.scopes} onChange={(scopeKind, scopeIds) => set({ scopeKind, scopeIds })} />
        </Step>

        <Step n={4} title="Recovery" hint="When the alarm clears" icon={LifeBuoy} tone="ok" done={done.rec}>
          <div className="grid gap-3 sm:grid-cols-2">
            <div className="space-y-1">
              <span className="block text-xs font-medium text-muted">{f.kind === "state" ? "Recovers when" : "Recovers past"}</span>
              {f.kind === "state" ? (
                <StateChips label="Recovery states" options={info?.states} value={f.recover} onChange={(recover) => set({ recover })} />
              ) : (
                <div className="flex items-center gap-1.5">
                  <span className="w-6 text-center text-xs text-muted">{f.cmp === "gt" || f.cmp === "ge" ? "<" : ">"}</span>
                  <Input aria-label="Recovery threshold" inputMode="decimal" className="w-24 tabular-nums" placeholder="optional" value={f.recoverAt} onChange={(e) => set({ recoverAt: e.target.value })} />
                  {sfx && <span className="text-xs text-muted">{sfx}</span>}
                </div>
              )}
              <span className="block text-xs text-faint">{f.kind === "state" ? "None chosen: when the condition no longer holds." : "Optional hysteresis; empty: when the condition no longer holds."}</span>
            </div>
            <Field label="Hold" hint="Recovery must last this long. 0: at once.">
              <DurationInput label="Hold" value={f.hold} onChange={(hold) => set({ hold })} />
            </Field>
          </div>
        </Step>

        <Step n={5} title="Enabled" icon={Power} tone={f.enabled ? "ok" : "warn"} done>
          <Switch checked={f.enabled} onChange={(enabled) => set({ enabled })} label={f.enabled ? "On: the rule evaluates and raises alarms" : "Off: the rule is kept but does nothing"} />
        </Step>
      </div>

      <div className="shrink-0 space-y-2 border-t border-line bg-panel px-5 py-3">
        <div className={`flex min-w-0 items-start gap-2 rounded-md border-l-[3px] px-2.5 py-1.5 text-xs ${err ? "border-warn bg-warn-soft" : "border-accent bg-accent-soft"}`}>
          <Sparkles aria-hidden className={`mt-0.5 size-3.5 shrink-0 ${err ? "text-warn" : "text-accent"}`} />
          <p className="min-w-0 break-words text-fg" aria-live="polite">
            <span className={`mr-1.5 inline-block rounded px-1 text-[11px] font-medium capitalize ${TONE[sevTone(f.severity)].chip}`}>{f.severity}</span>
            {summary(f, info?.unit)}
          </p>
        </div>
        {err && <p className="text-xs text-bad">{err}</p>}
        {save.isError && <FormError>{errorText(save.error)}</FormError>}
        <div className="flex justify-end gap-2">
          <Button type="button" variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" disabled={save.isPending || !!err} title={err}>
            {save.isPending && <Loader2 className="size-4 animate-spin" />}
            Save
          </Button>
        </div>
      </div>
    </form>
  );
}

/** Uploaded ingress certificates, for the scope picker (same key as the Certificates page). */
const useCertificateList = (enabled: boolean) =>
  useQuery({ queryKey: ["certificates"], queryFn: () => get<Certificate[]>("/api/v1/certificates"), enabled });

/** Which kind of resource the rule watches, and which ones: "All …" or chosen ones as chips. */
function ScopePicker({
  kind,
  ids,
  allowed,
  onChange,
}: {
  kind: RuleScopeKind;
  ids: string[];
  allowed?: RuleScopeKind[];
  onChange: (kind: RuleScopeKind, ids: string[]) => void;
}) {
  const nodes = useServers();
  const apps = useServices(undefined, undefined, kind === "services");
  const installs = useInstallations();
  const certs = useCertificateList(kind === "certificates");
  const host = (id: string) => {
    const n = (nodes.data ?? []).find((x) => x.id === id);
    return n ? nodeName(n) : short(id);
  };
  const items =
    kind === "servers"
      ? (nodes.data ?? []).map((n) => ({ id: n.id, label: nodeName(n) }))
      : kind === "services"
        ? (apps.data ?? []).map((a) => ({ id: a.id, label: a.name }))
        : kind === "installations"
          ? (installs.data ?? []).map((i) => ({ id: i.id, label: `${i.package} ${i.version} on ${host(i.server_id)}` }))
          : (certs.data ?? []).map((c) => ({ id: c.id, label: c.name }));
  const q = kind === "servers" ? nodes : kind === "services" ? apps : kind === "installations" ? installs : certs;
  const scope = SCOPES.find((s) => s.id === kind);
  const label = (id: string) => items.find((i) => i.id === id)?.label ?? `${short(id)} (not found)`;
  const rest = items.filter((i) => !ids.includes(i.id));
  return (
    <div className="grid gap-3 sm:grid-cols-[200px_1fr]">
      <Field label="Kind">
        <Select value={kind} onChange={(e) => onChange(e.target.value as RuleScopeKind, [])}>
          {SCOPES.filter((s) => !allowed || allowed.includes(s.id) || s.id === kind).map((s) => (
            <option key={s.id} value={s.id}>
              {s.label}
            </option>
          ))}
        </Select>
      </Field>
      <div className="min-w-0 space-y-1.5">
        <span className="block text-xs font-medium text-muted">{scope?.label ?? kind}</span>
        <div className="flex min-w-0 flex-wrap items-center gap-1.5">
          {ids.length === 0 ? (
            <span className="inline-flex items-center rounded-md border border-accent bg-accent-soft px-2 py-0.5 text-xs text-accent">{scope?.all ?? "All"}, including ones added later</span>
          ) : (
            <>
              <button type="button" className="rounded-md border border-line px-2 py-0.5 text-xs text-muted hover:text-fg" onClick={() => onChange(kind, [])}>
                {scope?.all ?? "All"}
              </button>
              {ids.map((id) => (
                <span key={id} className="inline-flex max-w-[220px] min-w-0 items-center gap-1 rounded-md border border-line bg-panel-2 py-0.5 pr-0.5 pl-2 text-xs">
                  <span className="truncate" title={label(id)}>
                    {label(id)}
                  </span>
                  <button type="button" aria-label={`Remove ${label(id)}`} className="grid size-4 place-items-center rounded text-muted hover:text-fg" onClick={() => onChange(kind, ids.filter((x) => x !== id))}>
                    <X className="size-3" />
                  </button>
                </span>
              ))}
            </>
          )}
        </div>
        {q.isLoading ? (
          <Skeleton rows={1} />
        ) : q.isError ? (
          <FormError>Could not load the list: {errorText(q.error)}. Chosen ones stay.</FormError>
        ) : items.length === 0 ? (
          <p className="text-xs text-faint">None yet; the rule covers ones added later.</p>
        ) : (
          rest.length > 0 && (
            <div className="max-w-xs">
              <Select aria-label={`Add ${scope?.label.toLowerCase() ?? "resources"}`} value="" onChange={(e) => e.target.value && onChange(kind, [...ids, e.target.value])}>
                <option value="">{ids.length ? "Add another…" : "Limit to chosen ones…"}</option>
                {rest.map((i) => (
                  <option key={i.id} value={i.id}>
                    {i.label}
                  </option>
                ))}
              </Select>
            </div>
          )
        )}
      </div>
    </div>
  );
}
