import * as Popover from "@radix-ui/react-popover";
import { Check, ChevronDown, Search } from "lucide-react";
import {
  Children,
  isValidElement,
  type ReactElement,
  type ReactNode,
  type SelectHTMLAttributes,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";

interface Opt {
  value: string;
  label: string;
  group?: string;
  disabled?: boolean;
}

function text(node: ReactNode): string {
  if (node == null || typeof node === "boolean") return "";
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(text).join("");
  if (isValidElement(node)) return text((node.props as { children?: ReactNode }).children);
  return "";
}

/** Reads `<option>` / `<optgroup>` children (fragments and arrays included). */
function collect(children: ReactNode, group?: string, out: Opt[] = []): Opt[] {
  Children.forEach(children, (c) => {
    if (!isValidElement(c)) return;
    const el = c as ReactElement<{ value?: string | number; children?: ReactNode; disabled?: boolean; label?: string }>;
    if (el.type === "option") {
      const label = text(el.props.children);
      out.push({ value: el.props.value === undefined ? label : String(el.props.value), label, group, disabled: el.props.disabled });
    } else if (el.type === "optgroup") {
      collect(el.props.children, el.props.label, out);
    } else if (el.props.children) {
      collect(el.props.children, group, out);
    }
  });
  return out;
}

/**
 * Drop-in for `<select>`: same props (`value`, `onChange(e)` with
 * `e.target.value`, `<option>`/`<optgroup>` children, `disabled`,
 * `required`), rendered as a compact button with a searchable list.
 * Every dropdown is searchable. Keyboard: type to filter, ↑/↓ to move,
 * Enter to pick, Esc to close. A hidden native select keeps form validation
 * (`required`) working.
 *
 * Built on Radix Popover, so it works inside dialogs and drawers: Radix
 * modals block pointer events and trap focus outside their content, which a
 * hand-made portal cannot get through (options could not be clicked).
 */
export function SearchSelect({ className = "", children, value, onChange, disabled, required, name, id, ...rest }: SelectHTMLAttributes<HTMLSelectElement>) {
  const opts = useMemo(() => collect(children), [children]);
  const current = String(value ?? "");
  const selected = opts.find((o) => o.value === current);
  const [open, setOpen] = useState(false);
  const [q, setQ] = useState("");
  const [active, setActive] = useState(0);
  const btn = useRef<HTMLButtonElement>(null);
  const list = useRef<HTMLDivElement>(null);

  const needle = q.trim().toLowerCase();
  const shown = needle ? opts.filter((o) => `${o.label} ${o.group ?? ""}`.toLowerCase().includes(needle)) : opts;

  useEffect(() => {
    if (!open) return;
    setQ("");
    // Start on the chosen option; with nothing chosen, highlight nothing
    // (the placeholder row must not look selected).
    setActive(opts.findIndex((o) => o.value === current && o.value !== ""));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);
  useEffect(() => {
    list.current?.querySelector(`[data-i="${active}"]`)?.scrollIntoView({ block: "nearest" });
  }, [active, open]);

  const pick = (o: Opt) => {
    if (o.disabled) return;
    setOpen(false);
    if (o.value !== current) onChange?.({ target: { value: o.value, name }, currentTarget: { value: o.value, name } } as unknown as React.ChangeEvent<HTMLSelectElement>);
  };
  const move = (d: number) => {
    if (!shown.length) return;
    let i = active < 0 ? (d > 0 ? -1 : 0) : active;
    for (let n = 0; n < shown.length; n++) {
      i = (i + d + shown.length) % shown.length;
      if (!shown[i]!.disabled) break;
    }
    setActive(i);
  };
  const onKey = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown") (e.preventDefault(), move(1));
    else if (e.key === "ArrowUp") (e.preventDefault(), move(-1));
    else if (e.key === "Enter") {
      e.preventDefault();
      const o = shown[active];
      if (o) pick(o);
    }
  };

  let lastGroup: string | undefined;
  return (
    <div className={`relative ${className.includes("w-") ? "" : "w-full"} ${className}`}>
      <Popover.Root open={open} onOpenChange={(o) => !disabled && setOpen(o)}>
        <Popover.Trigger asChild>
          <button
            ref={btn}
            type="button"
            id={id}
            disabled={disabled}
            aria-haspopup="listbox"
            aria-label={rest["aria-label"]}
            title={rest.title}
            onKeyDown={(e) => {
              if (["ArrowDown", "ArrowUp"].includes(e.key)) {
                e.preventDefault();
                setOpen(true);
              }
            }}
            className="flex h-8 w-full items-center gap-2 rounded-md border border-line bg-panel px-2.5 text-left text-[13px] transition-[border-color,box-shadow] hover:border-faint/60 focus-visible:border-accent focus-visible:shadow-[0_0_0_3px_color-mix(in_srgb,var(--color-accent)_18%,transparent)] focus-visible:outline-none disabled:cursor-not-allowed disabled:opacity-60 aria-expanded:border-accent"
          >
            <span className={`min-w-0 flex-1 truncate ${selected ? "" : "text-faint"}`}>{selected?.label ?? (opts[0]?.value === "" ? opts[0].label : "Select…")}</span>
            <ChevronDown className={`size-3.5 shrink-0 text-faint transition ${open ? "rotate-180" : ""}`} />
          </button>
        </Popover.Trigger>
        <Popover.Portal>
          <Popover.Content
            align="start"
            sideOffset={4}
            collisionPadding={8}
            onKeyDown={onKey}
            // Keep focus in the search box; return it to the button on close.
            onOpenAutoFocus={(e) => e.preventDefault()}
            onCloseAutoFocus={(e) => {
              e.preventDefault();
              btn.current?.focus();
            }}
            style={{ width: "max(var(--radix-popover-trigger-width), 180px)" }}
            className="z-[100] overflow-hidden rounded-md border border-line bg-panel text-[13px] shadow-[0_8px_28px_rgba(0,0,0,0.18)]"
          >
            <div className="flex items-center gap-1.5 border-b border-line px-2.5">
              <Search className="size-3.5 text-faint" />
              <input
                autoFocus
                data-bare
                value={q}
                onChange={(e) => (setQ(e.target.value), setActive(e.target.value.trim() ? 0 : -1))}
                placeholder="Search…"
                aria-label="Search options"
                className="h-8 w-full bg-transparent text-[13px] outline-none placeholder:text-faint"
              />
            </div>
            <div ref={list} role="listbox" className="max-h-64 overflow-y-auto py-1">
              {shown.map((o, i) => {
                const header = o.group && o.group !== lastGroup ? o.group : undefined;
                lastGroup = o.group;
                return (
                  <div key={`${o.group ?? ""}:${o.value}`}>
                    {header && <div className="px-2.5 pb-0.5 pt-1.5 text-[10.5px] font-semibold uppercase tracking-wide text-faint">{header}</div>}
                    <div
                      data-i={i}
                      role="option"
                      aria-selected={o.value === current}
                      aria-disabled={o.disabled}
                      onMouseEnter={() => setActive(i)}
                      onMouseDown={(e) => e.preventDefault()}
                      onClick={() => pick(o)}
                      className={`flex cursor-pointer items-center gap-2 px-2.5 py-1.5 ${o.group ? "pl-4" : ""} ${o.disabled ? "cursor-not-allowed opacity-45" : i === active ? "bg-accent-soft text-fg" : ""}`}
                    >
                      <span className={`min-w-0 flex-1 truncate ${o.value === "" ? "text-faint" : ""}`}>{o.label}</span>
                      {o.value === current && o.value !== "" && <Check className="size-3.5 shrink-0 text-accent" />}
                    </div>
                  </div>
                );
              })}
              {!shown.length && <div className="px-2.5 py-2 text-faint">No matches</div>}
            </div>
          </Popover.Content>
        </Popover.Portal>
      </Popover.Root>
      {/* Native mirror: form validation and autofill. */}
      <select
        tabIndex={-1}
        aria-hidden
        name={name}
        required={required}
        value={current}
        onChange={() => undefined}
        className="pointer-events-none absolute inset-x-0 bottom-0 h-px opacity-0"
      >
        {opts.map((o) => (
          <option key={o.value} value={o.value} disabled={o.disabled}>
            {o.label}
          </option>
        ))}
      </select>
    </div>
  );
}
