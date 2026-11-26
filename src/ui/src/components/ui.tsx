import * as Dialog from "@radix-ui/react-dialog";
import * as DropdownMenu from "@radix-ui/react-dropdown-menu";
import { useBlocker } from "@tanstack/react-router";
import { Check, ChevronLeft, ChevronRight, Copy, LayoutGrid, List, ListTree, Loader2, Lock, type LucideIcon, MoreHorizontal, Rows3, Search, X } from "lucide-react";
import { useEffect, useState, type ButtonHTMLAttributes, type InputHTMLAttributes, type ReactNode, type TdHTMLAttributes } from "react";
import { createPortal } from "react-dom";
import { useToast } from "../app/toast";
import { COPY_BLOCKED, copyText } from "../lib/clipboard";
import { SearchSelect } from "./SearchSelect";

// ------------------------------------------------------------------ formatting

export const time = (ms: number | null | undefined) => (ms ? new Date(ms).toLocaleString() : "—");

export function ago(ms: number | null | undefined): string {
  if (!ms) return "never";
  const s = Math.round((Date.now() - ms) / 1000);
  if (s < 0) return `in ${fmtSpan(-s)}`;
  if (s < 10) return "just now";
  return `${fmtSpan(s)} ago`;
}

function fmtSpan(s: number) {
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h`;
  return `${Math.floor(s / 86400)}d`;
}

/**
 * A compact event time for dense feeds: `HH:MM:SS.mmm` today, with the date
 * before it on other days. Pair it with `time()` in a tooltip.
 */
export function stamp(ms: number | null | undefined): string {
  if (!ms) return "—";
  const d = new Date(ms);
  const pad = (n: number, w = 2) => String(n).padStart(w, "0");
  const today = new Date().toDateString() === d.toDateString();
  const t = `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}.${pad(d.getMilliseconds(), 3)}`;
  return today ? t : `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${t}`;
}

export const short = (id: string | null | undefined) => (id ? id.slice(0, 8) : "—");

/** Elapsed time: 840ms, 4.2s, 3m 12s, 2h 05m; `—` when unknown or negative. */
export function duration(ms: number | null | undefined): string {
  if (ms === null || ms === undefined || !Number.isFinite(ms) || ms < 0) return "—";
  if (ms < 1000) return `${Math.round(ms)}ms`;
  const s = ms / 1000;
  if (s < 60) return `${s.toFixed(s < 10 ? 1 : 0)}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ${Math.floor(s % 60)}s`;
  return `${Math.floor(m / 60)}h ${String(m % 60).padStart(2, "0")}m`;
}

/** Binary size: 812 B, 12.4 KiB, 3.1 MiB, 1.2 GiB; `—` when unknown. */
export function bytes(n: number | null | undefined): string {
  if (n === null || n === undefined || !Number.isFinite(n) || n < 0) return "—";
  if (n < 1024) return `${n} B`;
  const units = ["KiB", "MiB", "GiB", "TiB", "PiB"];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toFixed(v < 10 ? 1 : 0)} ${units[i]}`;
}

/** Epoch ms or an ISO date string as epoch ms (undefined when unparseable). */
export function toMs(v: number | string | null | undefined): number | undefined {
  if (typeof v === "number") return v || undefined;
  if (!v) return undefined;
  const t = Date.parse(v);
  return Number.isNaN(t) ? undefined : t;
}

// ------------------------------------------------------------------ buttons

type Variant = "default" | "primary" | "danger" | "ghost";

export function Button({
  variant = "default",
  className = "",
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & { variant?: Variant }) {
  const styles: Record<Variant, string> = {
    default: "border border-line bg-panel hover:border-accent/60 hover:text-accent",
    primary: "btn-primary border border-transparent text-white shadow-sm enabled:hover:brightness-110 enabled:active:brightness-95",
    danger: "border border-bad/40 bg-bad-soft text-bad hover:border-bad",
    ghost: "border border-transparent text-muted hover:bg-panel-2 hover:text-fg",
  };
  return (
    <button
      className={`press inline-flex items-center justify-center gap-1.5 rounded-md px-3 py-1.5 text-[13px] font-medium focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-accent disabled:cursor-not-allowed disabled:opacity-50 ${styles[variant]} ${className}`}
      {...props}
    />
  );
}

export function IconButton({ label, className = "", ...props }: ButtonHTMLAttributes<HTMLButtonElement> & { label: string }) {
  return (
    <button
      aria-label={label}
      title={label}
      className={`press grid size-8 place-items-center rounded-md text-muted hover:bg-panel-2 focus-visible:outline-2 focus-visible:outline-accent hover:text-fg ${className}`}
      {...props}
    />
  );
}

/** Copies `text` and flashes a check; a blocked copy shows an error toast. */
function useCopy(text: string) {
  const [done, setDone] = useState(false);
  const toast = useToast();
  const copy = () => {
    void copyText(text).then((ok) => {
      if (!ok) {
        toast(COPY_BLOCKED, "error");
        return;
      }
      setDone(true);
      setTimeout(() => setDone(false), 1500);
    });
  };
  return { done, copy };
}

export function CopyButton({ text, label = "Copy" }: { text: string; label?: string }) {
  const { done, copy } = useCopy(text);
  return (
    <Button type="button" onClick={copy}>
      {done ? <Check className="size-4 text-ok" /> : <Copy className="size-4" />}
      {done ? "Copied" : label}
    </Button>
  );
}

/** Small inline copy icon for ids, hosts and commands next to their text. */
export function CopyIcon({ text, label = "Copy", className = "" }: { text: string; label?: string; className?: string }) {
  const { done, copy } = useCopy(text);
  return (
    <button
      type="button"
      aria-label={label}
      title={done ? "Copied" : label}
      onClick={(e) => {
        e.stopPropagation();
        copy();
      }}
      className={`inline-grid size-6 shrink-0 place-items-center rounded text-faint transition hover:bg-panel-2 hover:text-fg ${className}`}
    >
      {done ? <Check className="size-3.5 text-ok" /> : <Copy className="size-3.5" />}
    </button>
  );
}

/**
 * Click-to-copy value for hosts, ports, user names, one-time passwords,
 * fingerprints, endpoints and commands. The value itself is the button (hover
 * highlight, "Click to copy") with a copy icon at its end; Enter and Space copy
 * too. Long values truncate with the full value in the tooltip. Renders `—`
 * when there is nothing to copy.
 */
export function CopyValue({ text, label, className = "" }: { text: string; label?: string; className?: string }) {
  const { done, copy } = useCopy(text);
  if (!text) return <span className="font-mono text-xs text-faint">—</span>;
  return (
    <button
      type="button"
      aria-label={label ? `Copy ${label.toLowerCase()}` : "Copy"}
      title={done ? "Copied" : `${text}\nClick to copy`}
      onClick={(e) => {
        e.stopPropagation();
        copy();
      }}
      className={`group -mx-1 inline-flex max-w-full min-w-0 cursor-pointer items-center gap-1.5 rounded px-1 text-left font-mono text-xs transition hover:bg-accent-soft focus-visible:outline-2 focus-visible:outline-accent ${className}`}
    >
      <span className="min-w-0 truncate select-all">{text}</span>
      {done ? <Check className="size-3.5 shrink-0 text-ok" aria-hidden /> : <Copy className="size-3.5 shrink-0 text-faint group-hover:text-fg" aria-hidden />}
      <span className="sr-only" aria-live="polite">
        {done ? "Copied" : ""}
      </span>
    </button>
  );
}

// ------------------------------------------------------------------ status

const tone: Record<string, string> = {
  online: "ok",
  succeeded: "ok",
  paused: "warn",
  awaiting_approval: "warn",
  aborted: "muted",
  rolled_back: "warn",
  ready: "ok",
  started: "accent",
  executed: "ok",
  approved: "accent",
  expired: "muted",
  success: "ok",
  enabled: "ok",
  running: "accent",
  queued: "muted",
  pending: "muted",
  installed: "muted",
  disabled: "muted",
  cancelled: "muted",
  offline: "warn",
  failed: "bad",
  denied: "bad",
  failure: "bad",
  revoked: "bad",
  quarantined: "bad",
  retrying: "warn",
  connected: "ok",
  stopped: "muted",
  unknown: "muted",
  error: "bad",
  missing: "warn",
  installing: "accent",
  starting: "accent",
  stopping: "accent",
  restarting: "accent",
  checking: "accent",
  adopting: "accent",
  removing: "warn",
  degraded: "warn",
  removed: "muted",
};

const toneClass: Record<string, string> = {
  ok: "bg-ok-soft text-ok",
  accent: "bg-accent-soft text-accent",
  muted: "bg-panel-2 text-muted",
  warn: "bg-warn-soft text-warn",
  bad: "bg-bad-soft text-bad",
};

export function Status({ value }: { value: string }) {
  const t = tone[value] ?? "muted";
  return (
    <span className={`inline-flex items-center gap-1.5 rounded px-1.5 py-0.5 text-xs font-medium ${toneClass[t]}`}>
      <span aria-hidden className={`size-1.5 rounded-full bg-current ${value === "running" ? "animate-pulse" : ""}`} />
      {value}
    </span>
  );
}

export function Badge({ children, tone: t = "muted" }: { children: ReactNode; tone?: keyof typeof toneClass }) {
  return <span className={`inline-flex items-center rounded px-1.5 py-0.5 text-xs font-medium ${toneClass[t]}`}>{children}</span>;
}

export function Mono({ children }: { children: ReactNode }) {
  return <span className="font-mono text-xs">{children}</span>;
}

// ------------------------------------------------------------------ layout

/**
 * Compact page header: eyebrow and title, a one-line description (full text in
 * the tooltip), and actions (incl. a `ViewToggle`) on the title row.
 */
export function PageHeader({ title, description, actions, eyebrow }: { title: ReactNode; description?: string; actions?: ReactNode; eyebrow?: ReactNode }) {
  return (
    <div className="mb-3 flex flex-wrap items-center justify-between gap-x-4 gap-y-2">
      <div className="min-w-0 flex-1 basis-64">
        {eyebrow && <div className="truncate text-[10px] leading-4 font-semibold tracking-wider text-faint uppercase">{eyebrow}</div>}
        <h1 className="truncate text-lg leading-6 font-semibold tracking-tight">{title}</h1>
        {description && (
          <p className="truncate text-xs text-muted" title={description}>
            {description}
          </p>
        )}
      </div>
      {actions && <div className="flex flex-wrap items-center gap-2">{actions}</div>}
    </div>
  );
}

/**
 * One-line toolbar at the top of a tab panel: a short note on the left (full
 * text in the tooltip), actions on the right. Use it instead of a second page header.
 */
export function SectionBar({ description, actions }: { description?: string; actions?: ReactNode }) {
  return (
    <div className="mb-3 flex min-w-0 flex-wrap items-center justify-between gap-2">
      <p className="min-w-0 flex-1 basis-64 truncate text-xs text-muted" title={description}>
        {description}
      </p>
      {actions && <div className="flex flex-wrap items-center gap-2">{actions}</div>}
    </div>
  );
}

/**
 * Back link above a full-page form: "‹ Stacks › New stack". Pass the parent
 * as a router `<Link>` in `back` (typed routes stay checked at the call site).
 */
export function PageCrumb({ back, current }: { back: ReactNode; current: string }) {
  return (
    <nav aria-label="Breadcrumb" className="mb-2 flex min-w-0 items-center gap-1 text-xs text-muted">
      <span className="inline-flex shrink-0 items-center gap-0.5 [&_a]:rounded [&_a:hover]:text-fg">
        <ChevronLeft className="size-3.5" aria-hidden />
        {back}
      </span>
      <ChevronRight className="size-3 shrink-0 text-faint" aria-hidden />
      <span className="truncate text-fg" title={current}>
        {current}
      </span>
    </nav>
  );
}

/**
 * Sticky action bar at the bottom of a full-page form: a note on the left, buttons on the right.
 * `-bottom-5` cancels `<main>`'s bottom padding (`py-5`), which sticky positioning keeps clear,
 * so the bar sits flush with the bottom edge with no content showing below it.
 */
export function ActionBar({ note, children }: { note?: ReactNode; children: ReactNode }) {
  return (
    <div className="sticky -bottom-5 z-10 -mx-4 mt-4 -mb-5 flex flex-wrap items-center justify-end gap-2 border-t border-line bg-bg/95 px-4 py-2.5 backdrop-blur md:-mx-6 md:px-6">
      {note && <span className="mr-auto min-w-0 text-[11px] text-faint">{note}</span>}
      {children}
    </div>
  );
}

export function Panel({
  title,
  description,
  actions,
  children,
  className = "",
}: {
  title?: string;
  description?: ReactNode;
  actions?: ReactNode;
  children: ReactNode;
  className?: string;
}) {
  return (
    <section className={`rounded-lg border border-line bg-panel shadow-[var(--shadow-card)] ${className}`}>
      {(title || actions) && (
        <header className="card-head flex flex-wrap items-center justify-between gap-2 rounded-t-lg border-b border-line px-4 py-2.5">
          <div>
            {title && <h2 className="font-semibold">{title}</h2>}
            {description && <p className="text-xs text-muted">{description}</p>}
          </div>
          {actions}
        </header>
      )}
      {children}
    </section>
  );
}

export function Stat({ label, value, hint, icon, tone: t, onClick }: { label: string; value: ReactNode; hint?: string; icon?: ReactNode; tone?: "ok" | "warn" | "bad"; onClick?: () => void }) {
  const color = t === "ok" ? "text-ok" : t === "warn" ? "text-warn" : t === "bad" ? "text-bad" : "";
  return (
    <button
      onClick={onClick}
      disabled={!onClick}
      className="group rounded-lg border border-line bg-panel p-4 text-left shadow-[var(--shadow-card)] transition enabled:hover:border-accent/50"
    >
      <div className="flex items-center justify-between text-xs text-muted">
        {label}
        {icon && <span className="grid size-7 place-items-center rounded-md bg-accent-soft text-accent">{icon}</span>}
      </div>
      <div className={`mt-2 text-2xl font-semibold tracking-tight ${color}`}>{value}</div>
      {hint && <div className="mt-1 text-xs text-faint">{hint}</div>}
    </button>
  );
}

export function Empty({ icon, title, children, action }: { icon?: ReactNode; title: string; children?: ReactNode; action?: ReactNode }) {
  return (
    <div className="flex flex-col items-center px-6 py-12 text-center">
      {icon && <div className="mb-3 grid size-11 place-items-center rounded-full bg-accent-soft text-accent">{icon}</div>}
      <div className="font-medium">{title}</div>
      {children && <div className="mt-1 max-w-md text-muted">{children}</div>}
      {action && <div className="mt-4">{action}</div>}
    </div>
  );
}

/** Shown instead of a page the signed-in user's roles don't include. */
export function NoAccess() {
  return (
    <Empty icon={<Lock className="size-5" />} title="You don't have access to this page">
      Your roles don't include this area. Ask an administrator to grant you a role that does, or pick a page from the navigation.
    </Empty>
  );
}
export function ErrorNote({ error }: { error: unknown }) {
  if (!error) return null;
  return <p className="m-3 rounded-md bg-bad-soft px-3 py-2 text-bad">{error instanceof Error ? error.message : String(error)}</p>;
}

/** Class of an underlined tab trigger (Radix `Tabs.Trigger` or a button with `data-state`). */
export const tabTrigger =
  "press -mb-px flex shrink-0 items-center gap-1.5 border-b-2 border-transparent px-3 py-2 text-[13px] whitespace-nowrap text-muted hover:text-fg focus-visible:outline-2 focus-visible:outline-accent data-[state=active]:border-accent data-[state=active]:font-medium data-[state=active]:text-fg";

/** Fades its content in when `k` changes: wrap tab panels, view switches and loaded content. */
export function FadeSwitch({ k, children, className = "" }: { k: string; children: ReactNode; className?: string }) {
  return (
    <div key={k} className={`fade-in ${className}`}>
      {children}
    </div>
  );
}

export function Skeleton({ rows = 4 }: { rows?: number }) {
  return (
    <div className="space-y-2 p-4" aria-busy>
      {Array.from({ length: rows }, (_, i) => (
        <div key={i} className="h-8 animate-pulse rounded bg-panel-2" />
      ))}
    </div>
  );
}

// ------------------------------------------------------------------ tables

export function Table({ head, children }: { head: ReactNode[]; children: ReactNode }) {
  return (
    <div className="overflow-x-auto">
      <table className="w-full text-left">
        <thead className="bg-panel-2 text-[11px] tracking-wide text-muted uppercase">
          <tr>
            {head.map((h, i) => (
              <th key={i} className="px-4 py-2 font-medium whitespace-nowrap">
                {h}
              </th>
            ))}
          </tr>
        </thead>
        <tbody className="divide-y divide-line">{children}</tbody>
      </table>
    </div>
  );
}

export const Td = ({ children, className = "", ...rest }: { children?: ReactNode; className?: string } & TdHTMLAttributes<HTMLTableCellElement>) => (
  <td {...rest} className={`px-4 py-2.5 align-middle ${className}`}>
    {children}
  </td>
);

export function SearchInput({ value, onChange, placeholder = "Search…" }: { value: string; onChange: (v: string) => void; placeholder?: string }) {
  return (
    <label className="relative flex items-center">
      <Search className="pointer-events-none absolute left-2.5 size-4 text-faint" />
      <input
        className="w-56 rounded-md border border-line bg-panel py-1.5 pr-3 pl-8 outline-none focus:border-accent"
        placeholder={placeholder}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        aria-label={placeholder}
      />
    </label>
  );
}

// ------------------------------------------------------------------ forms

export function Field({ label, hint, children }: { label: string; hint?: ReactNode; children: ReactNode }) {
  return (
    <label className="block space-y-1">
      <span className="text-xs font-medium text-muted">{label}</span>
      {children}
      {hint && <span className="block text-xs text-faint">{hint}</span>}
    </label>
  );
}

const control = "w-full rounded-md border border-line bg-panel px-2.5 py-1.5 text-[13px] transition-[border-color,box-shadow] hover:border-faint/60 placeholder:text-faint disabled:opacity-60";
export const Input = (p: InputHTMLAttributes<HTMLInputElement>) => <input {...p} className={`${control} ${p.className ?? ""}`} />;
/** Searchable, compact dropdown; same props as `<select>`. */
export const Select = SearchSelect;
export const TextArea = (p: React.TextareaHTMLAttributes<HTMLTextAreaElement>) => (
  <textarea {...p} className={`${control} font-mono text-xs ${p.className ?? ""}`} />
);

/** On/off toggle (`role="switch"`), with its label beside it. */
export function Switch({ checked, onChange, label, disabled, title }: { checked: boolean; onChange: (v: boolean) => void; label: ReactNode; disabled?: boolean; title?: string }) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      disabled={disabled}
      title={title}
      onClick={() => onChange(!checked)}
      className="inline-flex items-center gap-2 rounded text-[13px] disabled:cursor-not-allowed disabled:opacity-60"
    >
      <span className={`relative inline-flex h-4 w-7 shrink-0 items-center rounded-full border transition ${checked ? "border-accent bg-accent" : "border-line-strong bg-panel-2"}`}>
        <span className={`size-3 rounded-full bg-panel shadow-sm transition-transform ${checked ? "translate-x-3.5" : "translate-x-0.5"}`} />
      </span>
      <span>{label}</span>
    </button>
  );
}

// ------------------------------------------------------------------ dialogs

export function Modal({
  open,
  onOpenChange,
  title,
  description,
  children,
  wide,
  width,
  fill,
}: {
  open: boolean;
  onOpenChange: (o: boolean) => void;
  title: string;
  description?: string;
  children: ReactNode;
  wide?: boolean;
  /** Explicit width in px (e.g. an editor beside a preview); overrides `wide`. */
  width?: number;
  /** The dialog does not scroll; its child is a flex column that scrolls its own body and pins a footer. */
  fill?: boolean;
}) {
  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="dlg-overlay fixed inset-0 z-40 bg-black/40 backdrop-blur-[2px]" />
        <Dialog.Content
          className={`dlg-content fixed top-[8vh] left-1/2 z-50 max-h-[84vh] -translate-x-1/2 ${fill ? "flex flex-col overflow-hidden" : "overflow-y-auto"} rounded-xl border border-line bg-panel p-5 shadow-[var(--shadow-pop)]`}
          style={{ width: `min(${width ?? (wide ? 720 : 480)}px, calc(100vw - 32px))` }}
        >
          <div className={`mb-4 flex items-start justify-between gap-4 ${fill ? "shrink-0" : ""}`}>
            <div>
              <Dialog.Title className="text-base font-semibold">{title}</Dialog.Title>
              <Dialog.Description className="mt-1 text-muted">{description ?? ""}</Dialog.Description>
            </div>
            <Dialog.Close aria-label="Close" className="text-muted hover:text-fg">
              <X className="size-5" />
            </Dialog.Close>
          </div>
          {fill ? <div className="flex min-h-0 flex-1 flex-col">{children}</div> : children}
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

export function Drawer({ open, onOpenChange, title, children, wide }: { open: boolean; onOpenChange: (o: boolean) => void; title: string; children: ReactNode; wide?: boolean }) {
  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="dlg-overlay fixed inset-0 z-40 bg-black/30" />
        <Dialog.Content className={`drawer-content fixed top-0 right-0 z-50 h-full ${wide ? "w-[min(880px,100vw)]" : "w-[min(480px,100vw)]"} overflow-y-auto border-l border-line bg-panel p-5 shadow-[var(--shadow-pop)]`}>
          <div className="mb-4 flex items-center justify-between">
            <Dialog.Title className="font-semibold">{title}</Dialog.Title>
            <Dialog.Close aria-label="Close" className="text-muted hover:text-fg">
              <X className="size-5" />
            </Dialog.Close>
          </div>
          <Dialog.Description className="sr-only">{title}</Dialog.Description>
          {children}
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

export function Confirm({
  open,
  onOpenChange,
  title,
  body,
  confirmLabel,
  onConfirm,
  busy,
  disabled,
  cancelLabel = "Cancel",
}: {
  open: boolean;
  onOpenChange: (o: boolean) => void;
  title: string;
  body: ReactNode;
  confirmLabel: string;
  onConfirm: () => void;
  busy?: boolean;
  /** Confirm is unavailable; say why in `body`. */
  disabled?: boolean;
  /** Label of the dismiss button (e.g. "Keep running" when the action itself is a cancel). */
  cancelLabel?: string;
}) {
  return (
    <Modal open={open} onOpenChange={onOpenChange} title={title}>
      <div className="text-muted">{body}</div>
      <div className="mt-5 flex justify-end gap-2">
        <Button onClick={() => onOpenChange(false)}>{cancelLabel}</Button>
        <Button variant="danger" onClick={onConfirm} disabled={busy || disabled}>
          {busy && <Loader2 className="size-4 animate-spin" />}
          {confirmLabel}
        </Button>
      </div>
    </Modal>
  );
}

export interface RowAction {
  label: string;
  icon?: LucideIcon;
  onSelect: () => void;
  disabled?: boolean;
  /** Why a disabled action is unavailable; shown under its label. */
  hint?: string;
  /** Destructive actions are shown in red, after a separator. */
  danger?: boolean;
  /** Highlight colour by the action's nature: `ok` start, `warn` restart, `bad` stop, `accent` logs and views. */
  tone?: ActionTone;
  hidden?: boolean;
}

/** Per-row "⋯" menu: replaces inline links and buttons at the end of table rows. Pair it with `useRowMenu` for right-click. */
export function RowMenu({ actions, label = "Actions" }: { actions: RowAction[]; label?: string }) {
  if (!actions.some((a) => !a.hidden)) return null;
  return (
    <DropdownMenu.Root modal={false}>
      <DropdownMenu.Trigger asChild>
        <button aria-label={label} className="press inline-grid size-8 place-items-center rounded-md text-muted hover:bg-panel-2 focus-visible:outline-2 focus-visible:outline-accent hover:text-fg data-[state=open]:bg-panel-2 data-[state=open]:text-fg">
          <MoreHorizontal className="size-4" />
        </button>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content align="end" sideOffset={4} className={menuContent}>
          <MenuItems actions={actions} />
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}

const menuContent = "pop-in z-50 min-w-[168px] rounded-lg border border-line bg-panel p-1 shadow-[var(--shadow-pop)]";

/** Where a context menu opens (viewport coordinates) and what it shows. */
export interface PointerMenuState {
  x: number;
  y: number;
  actions: RowAction[];
  label?: string;
}

/** The context-menu position for a mouse or keyboard (Shift+F10 / Menu key) event. */
export function menuPoint(ev: { clientX?: number; clientY?: number; currentTarget: Element }): { x: number; y: number } {
  if (ev.clientX || ev.clientY) return { x: ev.clientX ?? 0, y: ev.clientY ?? 0 };
  const r = ev.currentTarget.getBoundingClientRect();
  return { x: r.left + Math.min(r.width, 48), y: r.top + r.height };
}

/**
 * Right-click menu: the same items and styling as `RowMenu`, anchored at the pointer.
 * Built on the dropdown menu with an invisible trigger at the pointer, so no extra package.
 */
export function PointerMenu({ menu, onClose }: { menu: PointerMenuState | null; onClose: () => void }) {
  if (!menu || !menu.actions.some((a) => !a.hidden)) return null;
  return (
    <DropdownMenu.Root key={`${menu.x},${menu.y}`} open modal={false} onOpenChange={(o) => !o && onClose()}>
      {/* The anchor lives on <body>: under a transformed or filtered ancestor
          (animated panels), `position: fixed` is relative to that ancestor and
          the menu would open away from the pointer. */}
      {createPortal(
        <DropdownMenu.Trigger asChild>
          <span aria-hidden className="pointer-events-none fixed size-0" style={{ left: menu.x, top: menu.y }} />
        </DropdownMenu.Trigger>,
        document.body,
      )}
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          aria-label={menu.label}
          align="start"
          side="bottom"
          sideOffset={2}
          collisionPadding={8}
          onCloseAutoFocus={(e) => e.preventDefault()}
          className={menuContent}
        >
          <MenuItems actions={menu.actions} />
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}

/**
 * Right-click for table rows that have a `RowMenu`: spread `rowProps(actions, label)`
 * on the `<tr>` and render `menu` once. The same `actions` feed both menus.
 * The browser menu stays on links, form fields and selected text (open in a
 * new tab, paste, copy). Shift+F10 and the Menu key open it from a focused
 * button in the row (they fire `contextmenu` at the focused element).
 */
export function useRowMenu() {
  const [state, setState] = useState<PointerMenuState | null>(null);
  const rowProps = (actions: RowAction[], label?: string) => ({
    onContextMenu: (ev: React.MouseEvent<HTMLElement>) => {
      if (!actions.some((a) => !a.hidden)) return;
      const t = ev.target as HTMLElement | null;
      if (t?.closest("a[href],input,textarea,select,[contenteditable],.monaco-editor")) return;
      const sel = window.getSelection();
      if (sel && !sel.isCollapsed && sel.toString().trim() && ev.currentTarget.contains(sel.anchorNode)) return;
      ev.preventDefault();
      setState({ ...menuPoint(ev), actions, label });
    },
  });
  return { rowProps, menu: <PointerMenu menu={state} onClose={() => setState(null)} /> };
}

/** Menu body shared by `RowMenu` and `PointerMenu`: normal items, a separator, then destructive items in red. */
export type ActionTone = "ok" | "warn" | "bad" | "accent";

const toneHighlight: Record<ActionTone, string> = {
  ok: "data-[highlighted]:bg-ok-soft data-[highlighted]:text-ok",
  warn: "data-[highlighted]:bg-warn-soft data-[highlighted]:text-warn",
  bad: "data-[highlighted]:bg-bad-soft data-[highlighted]:text-bad",
  accent: "data-[highlighted]:bg-accent-soft data-[highlighted]:text-accent",
};

/** Hover colour of a lifecycle button or icon button by its nature; disabled buttons fade and stay grey. */
export const actionHover: Record<ActionTone, string> = {
  ok: "enabled:hover:bg-ok-soft enabled:hover:text-ok disabled:opacity-40 disabled:cursor-not-allowed",
  warn: "enabled:hover:bg-warn-soft enabled:hover:text-warn disabled:opacity-40 disabled:cursor-not-allowed",
  bad: "enabled:hover:bg-bad-soft enabled:hover:text-bad disabled:opacity-40 disabled:cursor-not-allowed",
  accent: "enabled:hover:bg-accent-soft enabled:hover:text-accent disabled:opacity-40 disabled:cursor-not-allowed",
};

function MenuItems({ actions }: { actions: RowAction[] }) {
  const shown = actions.filter((a) => !a.hidden);
  const normal = shown.filter((a) => !a.danger);
  const danger = shown.filter((a) => a.danger);
  const item = (a: RowAction) => (
    <DropdownMenu.Item
      key={a.label}
      disabled={a.disabled}
      onSelect={a.onSelect}
      title={a.disabled ? a.hint : undefined}
      className={`flex cursor-pointer items-center gap-2 rounded px-2.5 py-1.5 text-[13px] outline-none transition-colors duration-[var(--motion-fast)] select-none data-[disabled]:cursor-default ${a.disabled && a.hint ? "data-[disabled]:opacity-60" : "data-[disabled]:opacity-40"} ${
        a.danger ? "text-bad data-[highlighted]:bg-bad-soft" : a.tone && !a.disabled ? toneHighlight[a.tone] : "data-[highlighted]:bg-panel-2"
      }`}
    >
      {a.icon && <a.icon className="size-4 shrink-0 opacity-80" />}
      <span className="min-w-0 flex-1">
        {a.label}
        {a.disabled && a.hint && <span className="block max-w-[220px] text-[11px] leading-tight text-muted">{a.hint}</span>}
      </span>
    </DropdownMenu.Item>
  );
  return (
    <>
      {normal.map(item)}
      {normal.length > 0 && danger.length > 0 && <DropdownMenu.Separator className="my-1 h-px bg-line" />}
      {danger.map(item)}
    </>
  );
}

// ------------------------------------------------------------------ views

export type ViewMode = "list" | "cards" | "explorer";

/** One layout a page offers in its `ViewToggle`. */
export interface ViewOption<V extends string = string> {
  id: V;
  label: string;
  icon: LucideIcon;
}

export const VIEW_LIST: ViewOption<"list"> = { id: "list", label: "List", icon: List };
export const VIEW_CARDS: ViewOption<"cards"> = { id: "cards", label: "Cards", icon: LayoutGrid };
export const VIEW_EXPLORER: ViewOption<"explorer"> = { id: "explorer", label: "Explorer", icon: ListTree };
/** A grouped layout ("By cluster", "By environment"). */
export const groupedView = <V extends string>(id: V, label: string, icon: LucideIcon = Rows3): ViewOption<V> => ({ id, label, icon });

function readPref(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function writePref(key: string, v: string) {
  try {
    localStorage.setItem(key, v);
  } catch {
    /* private mode: keep it for this visit */
  }
}

/** Remembered **List / Cards** choice for a catalog-style page (per browser). */
export function useViewPref(key: string, fallback: ViewMode = "list"): [ViewMode, (v: ViewMode) => void] {
  const [view, setViewState] = useState<ViewMode>(() => {
    const v = readPref(key);
    return v === "cards" || v === "list" || v === "explorer" ? v : fallback;
  });
  const setView = (v: ViewMode) => {
    setViewState(v);
    writePref(key, v);
  };
  return [view, setView];
}

/**
 * A page's layout mode: `?view=` wins, else the remembered choice (per page,
 * per browser), else the first option. Choosing a mode remembers it and drops
 * `?view=` from the URL (the remembered choice then applies).
 */
export function usePageView<V extends string>(
  key: string,
  options: readonly ViewOption<V>[],
  url: string | undefined,
  setUrl: (v: V | undefined) => void,
): [V, (v: V) => void] {
  const valid = (v: string | null | undefined) => options.find((o) => o.id === v)?.id;
  const [pref, setPref] = useState<V | undefined>(() => valid(readPref(key)));
  const view = valid(url) ?? pref ?? options[0]?.id ?? ("list" as V);
  const setView = (v: V) => {
    setPref(v);
    writePref(key, v);
    setUrl(undefined);
  };
  return [view, setView];
}

const BUILTIN_VIEWS: Record<string, ViewOption> = { list: VIEW_LIST, cards: VIEW_CARDS, explorer: VIEW_EXPLORER };

/** Layout segmented control (List · Cards · By cluster …); pair it with `usePageView`. */
export function ViewToggle<V extends string>({ view, onChange, modes = ["list", "cards"] as unknown as readonly V[] }: { view: V; onChange: (v: V) => void; modes?: readonly (V | ViewOption<V>)[] }) {
  const opts = modes.map((m) => (typeof m === "string" ? ((BUILTIN_VIEWS[m] as ViewOption<V> | undefined) ?? { id: m, label: m, icon: List }) : m));
  return (
    <div className="inline-flex shrink-0 rounded-lg border border-line bg-panel p-0.5" role="group" aria-label="Layout">
      {opts.map(({ id, icon: Icon, label }) => (
        <button
          key={id}
          type="button"
          aria-pressed={view === id}
          onClick={() => onChange(id)}
          className={`press inline-flex h-7 items-center gap-1.5 rounded-md px-2.5 text-xs whitespace-nowrap focus-visible:outline-2 focus-visible:outline-accent ${view === id ? "bg-accent-soft font-medium text-accent" : "text-muted hover:text-fg"}`}
        >
          <Icon className="size-3.5" aria-hidden /> {label}
        </button>
      ))}
    </div>
  );
}

/** One option of a `Segmented` control; `count` shows as a small number. */
export interface SegmentOption<S extends string> {
  id: S;
  label: ReactNode;
  count?: number;
  title?: string;
}

/** Compact segmented control (status tabs, Live / History). */
export function Segmented<S extends string>({ label, value, options, onChange }: { label: string; value: S; options: readonly SegmentOption<S>[]; onChange: (v: S) => void }) {
  return (
    <div role="tablist" aria-label={label} className="flex shrink-0 gap-0.5 overflow-x-auto rounded-md bg-panel-2 p-0.5">
      {options.map((o) => (
        <button
          key={o.id}
          type="button"
          role="tab"
          aria-selected={value === o.id}
          title={o.title}
          onClick={() => onChange(o.id)}
          className={`press flex h-6 items-center gap-1.5 rounded px-2.5 text-xs whitespace-nowrap focus-visible:outline-2 focus-visible:outline-accent ${value === o.id ? "bg-panel font-medium text-fg shadow-sm" : "text-muted hover:text-fg"}`}
        >
          {o.label}
          {o.count !== undefined && <span className={`rounded px-1 font-mono text-[10px] tabular-nums ${value === o.id ? "bg-accent-soft text-accent" : "text-faint"}`}>{o.count}</span>}
        </button>
      ))}
    </div>
  );
}

/** One collapsible group of `GroupedSections`. */
export interface Section<T> {
  id: string;
  title: ReactNode;
  /** Right of the title: counts such as "3/4 online". */
  meta?: ReactNode;
  actions?: ReactNode;
  items: T[];
}

/**
 * Grouped layout: one collapsible section per group, each rendering the page's
 * own table rows through `children`. Empty groups are skipped.
 */
export function GroupedSections<T>({ sections, children, label = "Groups" }: { sections: Section<T>[]; children: (items: T[], section: Section<T>) => ReactNode; label?: string }) {
  const [closed, setClosed] = useState<Set<string>>(() => new Set());
  const flip = (id: string) =>
    setClosed((s) => {
      const n = new Set(s);
      if (n.has(id)) n.delete(id);
      else n.add(id);
      return n;
    });
  return (
    <div className="divide-y divide-line" aria-label={label}>
      {sections
        .filter((s) => s.items.length > 0)
        .map((s) => {
          const open = !closed.has(s.id);
          return (
            <section key={s.id}>
              <div className="flex min-w-0 items-center gap-2 bg-panel-2/60 px-3 py-1.5">
                <button type="button" onClick={() => flip(s.id)} aria-expanded={open} className="press flex min-w-0 flex-1 items-center gap-1.5 rounded text-left text-[13px] font-medium focus-visible:outline-2 focus-visible:outline-accent">
                  <ChevronRight className={`size-3.5 shrink-0 text-muted transition-transform duration-[var(--motion-fast)] ${open ? "rotate-90" : ""}`} aria-hidden />
                  <span className="min-w-0 truncate">{s.title}</span>
                  <span className="shrink-0 font-mono text-[11px] font-normal text-faint tabular-nums">{s.meta ?? s.items.length}</span>
                </button>
                {s.actions}
              </div>
              {open && <div className="fade-in">{children(s.items, s)}</div>}
            </section>
          );
        })}
    </div>
  );
}

/**
 * Sticky bar at the bottom of the content area, shown only while a form has
 * unsaved changes (draft differs from the server value). Navigating away while
 * dirty asks first. Without `onSave`, Save submits the enclosing form (or the one named by `form`).
 */
export function UnsavedBar({
  dirty,
  onDiscard,
  onSave,
  form,
  saving,
  saveLabel = "Save",
  saveDisabled,
  summary,
  extra,
}: {
  dirty: boolean;
  onDiscard: () => void;
  onSave?: () => void;
  /** Id of the `<form>` Save submits when there is no `onSave` and the bar sits outside it. */
  form?: string;
  saving?: boolean;
  saveLabel?: string;
  /** Save is unavailable (e.g. invalid input); say why in `summary`. */
  saveDisabled?: boolean;
  /** Validation summary shown after "Unsaved changes". */
  summary?: ReactNode;
  /** Extra action left of Discard. */
  extra?: ReactNode;
}) {
  // Only leaving the page asks; tab and filter changes (search params) don't.
  const blocker = useBlocker({ shouldBlockFn: ({ current, next }) => dirty && current.pathname !== next.pathname, enableBeforeUnload: () => dirty, withResolver: true });
  return (
    <>
      {/* `-bottom-5` cancels `<main>`'s bottom padding so the bar sits on the bottom edge, as ActionBar does */}
      {dirty && (
        <div role="region" aria-label="Unsaved changes" className="bar-in sticky -bottom-5 z-20 mt-3 flex flex-wrap items-center justify-end gap-2 rounded-t-lg border border-b-0 border-line-strong bg-panel/95 px-3 py-1.5 shadow-[var(--shadow-pop)] backdrop-blur">
          <span className="mr-auto flex min-w-0 items-center gap-2 text-xs">
            <span className="size-1.5 shrink-0 rounded-full bg-warn" aria-hidden />
            <span className="font-medium">Unsaved changes</span>
            {summary && <span className="min-w-0 truncate text-bad">{summary}</span>}
          </span>
          {extra}
          <Button type="button" variant="ghost" onClick={onDiscard} disabled={saving}>
            Discard
          </Button>
          <Button type={onSave ? "button" : "submit"} form={form} variant="primary" onClick={onSave} disabled={saving || saveDisabled}>
            {saving && <Loader2 className="size-4 animate-spin" />}
            {saveLabel}
          </Button>
        </div>
      )}
      {blocker.status === "blocked" && (
        <Confirm
          open
          onOpenChange={(o) => !o && blocker.reset()}
          title="Leave without saving?"
          body="You have unsaved changes on this page. Leaving discards them."
          confirmLabel="Discard and leave"
          cancelLabel="Stay"
          onConfirm={() => blocker.proceed()}
        />
      )}
    </>
  );
}

/** A value that follows `value` after it stopped changing for `ms` (search boxes, live previews). */
export function useDebounced<T>(value: T, ms: number): T {
  const [v, setV] = useState(value);
  useEffect(() => {
    const t = setTimeout(() => setV(value), ms);
    return () => clearTimeout(t);
  }, [value, ms]);
  return v;
}
