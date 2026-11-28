// Full-page form layout for forms too large for a modal: header with breadcrumb, a sticky
// section nav with per-section state, section cards with a tinted icon tile, an optional
// live summary panel, and a sticky action bar. Stacks into one column at ≤720px.
import { AlertCircle, CircleCheck, Circle, type LucideIcon } from "lucide-react";
import type { FormEvent, ReactNode } from "react";

import { ActionBar, PageCrumb, PageHeader } from "./ui";

/** Tinted icon tiles, in the console's job colour language (`JobVisuals`). */
export const TILE = {
  accent: "bg-accent-soft text-accent",
  ok: "bg-ok-soft text-ok",
  warn: "bg-warn-soft text-warn",
  violet: "bg-violet-soft text-violet",
  teal: "bg-teal-soft text-teal",
  sky: "bg-sky-soft text-sky",
  indigo: "bg-indigo-soft text-indigo",
  orange: "bg-orange-soft text-orange",
  pink: "bg-pink-soft text-pink",
} as const;
export type TileTone = keyof typeof TILE;

export type SectionState = "idle" | "ok" | "error";

export interface FormSectionMeta {
  id: string;
  title: string;
  icon: LucideIcon;
  tone: TileTone;
  state?: SectionState;
}

const sectionDom = (id: string) => `form-section-${id}`;

function StateIcon({ state }: { state?: SectionState }) {
  if (state === "error") return <AlertCircle aria-label="Has problems" className="size-3.5 shrink-0 text-bad" />;
  if (state === "ok") return <CircleCheck aria-label="Complete" className="size-3.5 shrink-0 text-ok" />;
  return <Circle aria-hidden className="size-3.5 shrink-0 text-faint" />;
}

/**
 * The page: a `PageCrumb` (`back` link, `current` label), `PageHeader`, the sections, and an
 * `ActionBar` with `footer` (and `note` on its left). `summary` is a live preview beside the
 * sections (below them on narrow screens).
 */
export function FormPage({
  back,
  current,
  title,
  description,
  sections,
  children,
  summary,
  footer,
  note,
  onSubmit,
}: {
  back: ReactNode;
  current: string;
  title: ReactNode;
  description?: string;
  sections?: FormSectionMeta[];
  children: ReactNode;
  summary?: ReactNode;
  footer?: ReactNode;
  note?: ReactNode;
  onSubmit?: () => void;
}) {
  const nav = sections ?? [];
  const body = (
    <>
      <div className={`grid min-w-0 gap-4 ${nav.length ? "min-[721px]:grid-cols-[11rem_minmax(0,1fr)]" : ""} ${summary ? (nav.length ? "xl:grid-cols-[11rem_minmax(0,1fr)_18rem]" : "xl:grid-cols-[minmax(0,1fr)_18rem]") : ""}`}>
        {nav.length > 0 && (
          <nav aria-label="Sections" className="min-w-0 min-[721px]:sticky min-[721px]:top-0 min-[721px]:self-start">
            <ol className="flex gap-1 overflow-x-auto min-[721px]:flex-col">
              {nav.map((s, i) => (
                <li key={s.id} className="shrink-0">
                  <a
                    href={`#${sectionDom(s.id)}`}
                    onClick={(e) => {
                      e.preventDefault();
                      document.getElementById(sectionDom(s.id))?.scrollIntoView({ behavior: "smooth", block: "start" });
                    }}
                    className={`flex items-center gap-2 rounded-md px-2 py-1.5 text-xs hover:bg-panel-2 focus-visible:bg-panel-2 ${s.state === "error" ? "text-bad" : "text-muted hover:text-fg"}`}
                  >
                    <span className={`grid size-5 shrink-0 place-items-center rounded ${TILE[s.tone]}`}>
                      <s.icon className="size-3" />
                    </span>
                    <span className="min-w-0 flex-1 truncate" title={s.title}>
                      <span className="mr-1 font-mono text-[10px] text-faint tabular-nums">{i + 1}</span>
                      {s.title}
                    </span>
                    <StateIcon state={s.state} />
                  </a>
                </li>
              ))}
            </ol>
          </nav>
        )}
        <div className="min-w-0 space-y-3">{children}</div>
        {summary && <aside className="min-w-0 xl:sticky xl:top-0 xl:self-start">{summary}</aside>}
      </div>
      {footer && <ActionBar note={note}>{footer}</ActionBar>}
    </>
  );
  return (
    <div className="min-w-0">
      <PageCrumb back={back} current={current} />
      <PageHeader title={title} description={description} />
      {onSubmit ? (
        <form
          noValidate
          onSubmit={(e: FormEvent) => {
            e.preventDefault();
            onSubmit();
          }}
        >
          {body}
        </form>
      ) : (
        body
      )}
    </div>
  );
}

/** A section card: tinted icon tile, title, one plain sentence, then the fields. */
export function FormSection({ id, title, description, icon: Icon, tone, children, aside }: { id: string; title: string; description?: ReactNode; icon: LucideIcon; tone: TileTone; children: ReactNode; aside?: ReactNode }) {
  return (
    <section id={sectionDom(id)} aria-labelledby={`${sectionDom(id)}-t`} className="scroll-mt-2 rounded-lg border border-line bg-panel">
      <div className="flex items-start gap-3 border-b border-line px-4 py-3">
        <span aria-hidden className={`grid size-8 shrink-0 place-items-center rounded-md ${TILE[tone]}`}>
          <Icon className="size-4" />
        </span>
        <div className="min-w-0 flex-1">
          <h2 id={`${sectionDom(id)}-t`} className="text-[13px] font-semibold">
            {title}
          </h2>
          {description && <p className="text-xs text-muted">{description}</p>}
        </div>
        {aside}
      </div>
      <div className="space-y-4 px-4 py-4">{children}</div>
    </section>
  );
}

export interface Choice<T extends string> {
  id: T;
  title: string;
  description: string;
  icon: LucideIcon;
  /** Why it cannot be chosen; disables the card and shows the reason. */
  why?: string | null;
  badge?: ReactNode;
}

/** Radio group as cards: icon, title and a short description; the selected one is coloured. */
export function ChoiceCards<T extends string>({ label, value, onChange, options, tone = "accent", disabled }: { label: string; value: T; onChange: (v: T) => void; options: Choice<T>[]; tone?: TileTone; disabled?: boolean }) {
  return (
    <div role="radiogroup" aria-label={label} className={`grid gap-2 ${options.length >= 3 ? "sm:grid-cols-3" : "sm:grid-cols-2"}`}>
      {options.map((o) => {
        const on = value === o.id;
        const off = disabled || !!o.why;
        return (
          <label
            key={o.id}
            title={o.why ?? undefined}
            className={`relative flex min-w-0 items-start gap-2.5 rounded-lg border px-3 py-2.5 transition-colors has-[:focus-visible]:ring-2 has-[:focus-visible]:ring-accent/50 ${
              on ? "border-accent bg-accent-soft/60" : "border-line bg-panel hover:border-line-strong"
            } ${off ? "cursor-not-allowed opacity-60" : "cursor-pointer"}`}
          >
            <input type="radio" className="sr-only" name={label} checked={on} disabled={off} onChange={() => onChange(o.id)} />
            <span aria-hidden className={`grid size-8 shrink-0 place-items-center rounded-md ${on ? TILE[tone] : "bg-panel-2 text-muted"}`}>
              <o.icon className="size-4" />
            </span>
            <span className="min-w-0 flex-1">
              <span className="flex flex-wrap items-center gap-1.5 text-[13px] font-medium">
                {o.title}
                {o.badge}
              </span>
              <span className="block text-xs text-muted">{o.description}</span>
              {o.why && <span className="mt-0.5 block text-xs text-warn">{o.why}</span>}
            </span>
            {on && <CircleCheck aria-hidden className="size-4 shrink-0 text-accent" />}
          </label>
        );
      })}
    </div>
  );
}

/** Inline validation message under a field. */
export function FieldError({ children }: { children?: ReactNode }) {
  if (!children) return null;
  return (
    <span role="alert" className="flex items-start gap-1 text-xs text-bad">
      <AlertCircle aria-hidden className="mt-px size-3.5 shrink-0" />
      <span className="min-w-0">{children}</span>
    </span>
  );
}

/** Summary panel shell. */
export function SummaryPanel({ title = "Summary", children }: { title?: string; children: ReactNode }) {
  return (
    <div className="space-y-3 rounded-lg border border-line bg-panel px-3 py-3">
      <div className="text-[10px] font-semibold tracking-wider text-faint uppercase">{title}</div>
      {children}
    </div>
  );
}

/** One label/value line of the summary; values wrap, never overflow. */
export function SummaryRow({ label, children, mono }: { label: string; children: ReactNode; mono?: boolean }) {
  return (
    <div className="grid grid-cols-[5.5rem_minmax(0,1fr)] gap-2 text-xs">
      <span className="text-muted">{label}</span>
      <span className={`min-w-0 break-all ${mono ? "font-mono text-[11px] tabular-nums" : ""}`}>{children ?? "—"}</span>
    </div>
  );
}
