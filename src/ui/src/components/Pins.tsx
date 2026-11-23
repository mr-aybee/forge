import * as DropdownMenu from "@radix-ui/react-dropdown-menu";
import { Link } from "@tanstack/react-router";
import { AppWindow, ArrowDown, ArrowUp, Boxes, ChevronRight, Loader2, MoreHorizontal, Pin as PinIcon, PinOff, Server, ServerCog, X, type LucideIcon } from "lucide-react";
import type { KeyboardEvent } from "react";

import { useMyCapabilities } from "../api/access";
import { ApiError } from "../api/client";
import { useServers } from "../api/hooks";
import { PIN_KINDS, PIN_LIMIT, type Pin, type PinKind, type PinRef, pinTarget, useIsPinned, usePin, usePins, useReorderPins, useUnpin } from "../api/pins";
import type { AgentStatus } from "../api/types";
import { errorText, useToast } from "../app/toast";
import type { RowAction } from "./ui";

export const PIN_ICON: Record<PinKind, LucideIcon> = { project: AppWindow, service: Boxes, node: Server, cluster: ServerCog };
const KIND_LABEL: Record<PinKind, string> = { project: "Projects", service: "Services", node: "Nodes", cluster: "Clusters" };

/** Why pinning failed, in words. */
function pinError(e: unknown): string {
  if (e instanceof ApiError && (e.status === 409 || e.status === 422)) return `You can pin up to ${PIN_LIMIT} items. Unpin one first.`;
  if (e instanceof ApiError && e.status === 404) return "This item no longer exists.";
  return errorText(e);
}

/** Pin or unpin with toasts. `available` is false on servers without pins. */
export function usePinToggle(ref: PinRef) {
  const toast = useToast();
  const state = useIsPinned(ref.kind, ref.id || undefined);
  const pin = usePin();
  const unpin = useUnpin();
  const busy = pin.isPending || unpin.isPending;
  const toggle = () => {
    if (state.pinned)
      unpin.mutate(ref, { onSuccess: () => toast(`Unpinned ${ref.label ?? ref.id}`), onError: (e) => toast(errorText(e), "error") });
    else pin.mutate(ref, { onSuccess: () => toast(`Pinned ${ref.label ?? ref.id} to the sidebar`), onError: (e) => toast(pinError(e), "error") });
  };
  const disabledWhy = !state.pinned && state.full ? `You can pin up to ${PIN_LIMIT} items` : undefined;
  return { ...state, busy, toggle, disabledWhy };
}

/** Header button: Pin / Unpin. Hidden when the server has no pins. */
export function PinToggle(ref: PinRef) {
  const t = usePinToggle(ref);
  if (!t.available) return null;
  const label = t.pinned ? "Unpin from sidebar" : "Pin to sidebar";
  const Icon = t.busy ? Loader2 : t.pinned ? PinOff : PinIcon;
  return (
    <button
      type="button"
      onClick={t.toggle}
      disabled={t.busy || !!t.disabledWhy}
      aria-pressed={t.pinned}
      aria-label={label}
      title={t.disabledWhy ?? label}
      className={`press inline-grid size-8 place-items-center rounded-md hover:bg-panel-2 focus-visible:outline-2 focus-visible:outline-accent disabled:opacity-50 ${t.pinned ? "text-accent" : "text-muted hover:text-fg"}`}
    >
      <Icon className={`size-4 ${t.busy ? "animate-spin" : ""}`} />
    </button>
  );
}

/** A row-menu entry for `ref`; hidden when pins are unavailable. */
export function usePinAction(): (ref: PinRef) => RowAction {
  const toast = useToast();
  const pins = usePins().data;
  const pin = usePin();
  const unpin = useUnpin();
  const items = pins?.items ?? [];
  return (ref) => {
    const pinned = items.some((p) => p.kind === ref.kind && p.id === ref.id);
    const full = !pinned && items.length >= PIN_LIMIT;
    return {
      label: pinned ? "Unpin from sidebar" : "Pin to sidebar",
      icon: pinned ? PinOff : PinIcon,
      hidden: pins?.available !== true,
      disabled: full,
      hint: full ? `Up to ${PIN_LIMIT} pins` : undefined,
      onSelect: () =>
        pinned
          ? unpin.mutate(ref, { onSuccess: () => toast(`Unpinned ${ref.label ?? ref.id}`), onError: (e) => toast(errorText(e), "error") })
          : pin.mutate(ref, { onSuccess: () => toast(`Pinned ${ref.label ?? ref.id} to the sidebar`), onError: (e) => toast(pinError(e), "error") }),
    };
  };
}

/* ------------------------------------------------------------ sidebar */

const dotFor: Partial<Record<AgentStatus, string>> = { online: "bg-ok", offline: "bg-bad", pending: "bg-warn" };

/** Node status bullets for pinned nodes, from the (shared) servers query. */
function useNodeDots(): (id: string) => { cls: string; status: string } | undefined {
  const fleet = useMyCapabilities().data?.fleet_view === true;
  const servers = useServers(fleet).data;
  return (id) => {
    const s = Array.isArray(servers) ? servers.find((x) => x?.id === id) : undefined;
    const cls = s ? dotFor[s.status] : undefined;
    return s && cls ? { cls, status: s.status } : undefined;
  };
}

/** Group pins by kind only when it helps: many pins spread over several kinds. */
const shouldGroup = (items: Pin[]) => items.length > 6 && new Set(items.map((p) => p.kind)).size > 1;

interface PinnedNavProps {
  open: Set<string>;
  setGroup: (id: string, value: boolean) => void;
  isActive: (to: string) => boolean;
}

/** "Pinned" at the top of the expanded sidebar. Hidden on servers without pins. */
export function PinnedNav({ open, setGroup, isActive }: PinnedNavProps) {
  const pins = usePins().data;
  const toast = useToast();
  const unpin = useUnpin();
  const reorder = useReorderPins();
  const dot = useNodeDots();
  if (!pins?.available) return null;
  const items = pins.items;
  const isOpen = open.has("pins");

  // Move within the list (or within the kind when grouped): swap with the neighbour.
  const move = (p: Pin, dir: -1 | 1, scope: Pin[]) => {
    const i = scope.indexOf(p);
    const other = scope[i + dir];
    if (i < 0 || !other) return;
    const next = [...items];
    const a = next.indexOf(p);
    const b = next.indexOf(other);
    next[a] = other;
    next[b] = p;
    reorder.mutate(next, { onError: (e) => toast(errorText(e), "error") });
  };
  const remove = (p: Pin) => unpin.mutate(p, { onError: (e) => toast(errorText(e), "error") });

  const rows = (list: Pin[], parent: string) =>
    list.map((p, i) => (
      <PinRow
        key={`${p.kind}:${p.id}`}
        p={p}
        parent={parent}
        active={isActive(pinTarget(p).to)}
        dot={p.kind === "node" ? dot(p.id) : undefined}
        canUp={i > 0}
        canDown={i < list.length - 1}
        onMove={(d) => move(p, d, list)}
        onUnpin={() => remove(p)}
      />
    ));

  return (
    <div role="group" aria-label="Pinned" className="mb-3 space-y-0.5">
      <button
        type="button"
        data-navkey
        data-group="pins"
        aria-expanded={isOpen}
        aria-controls="nav-pins"
        onClick={() => setGroup("pins", !isOpen)}
        className="flex w-full min-w-0 items-center gap-1.5 rounded-md px-2 pt-1 pb-1 text-left text-[10px] font-semibold tracking-widest text-faint uppercase transition hover:text-fg"
      >
        <PinIcon className="size-3 shrink-0" />
        <span className="min-w-0 flex-1 truncate">Pinned</span>
        {items.length > 0 && <span className="font-mono tabular-nums">{items.length}</span>}
        <ChevronRight className={`size-3.5 shrink-0 transition ${isOpen ? "rotate-90" : ""}`} />
      </button>
      {isOpen && (
        <div id="nav-pins" className="space-y-0.5">
          {items.length === 0 ? (
            <p className="px-2 py-1 text-[11px] leading-snug text-faint">Pin a project, service, node or cluster from its page.</p>
          ) : shouldGroup(items) ? (
            PIN_KINDS.map((k) => {
              const list = items.filter((p) => p.kind === k);
              if (!list.length) return null;
              const gid = `pins.${k}`;
              const subOpen = open.has(gid);
              const Icon = PIN_ICON[k];
              return (
                <div key={k} role="none">
                  <button
                    type="button"
                    data-navkey
                    data-group={gid}
                    data-parent="pins"
                    aria-expanded={subOpen}
                    aria-controls={`nav-${gid}`}
                    onClick={() => setGroup(gid, !subOpen)}
                    className="flex w-full min-w-0 items-center gap-2.5 rounded-md py-1 pr-2 pl-2 text-left text-[12.5px] text-muted transition hover:bg-panel-2 hover:text-fg"
                  >
                    <Icon className="size-3.5 shrink-0" />
                    <span className="min-w-0 flex-1 truncate">{KIND_LABEL[k]}</span>
                    <span className="font-mono text-[10px] text-faint tabular-nums">{list.length}</span>
                    <ChevronRight className={`size-3.5 shrink-0 text-faint transition ${subOpen ? "rotate-90" : ""}`} />
                  </button>
                  {subOpen && (
                    <div id={`nav-${gid}`} role="group" aria-label={KIND_LABEL[k]} className="ml-[15px] space-y-0.5 border-l border-line/70 pl-1">
                      {rows(list, gid)}
                    </div>
                  )}
                </div>
              );
            })
          ) : (
            rows(items, "pins")
          )}
        </div>
      )}
    </div>
  );
}

const menuContent = "animate-in z-50 min-w-[168px] rounded-lg border border-line bg-panel p-1 text-[13px] shadow-[var(--shadow-pop)]";
const menuItem =
  "flex cursor-pointer items-center gap-2 rounded-md px-2 py-1.5 text-muted outline-none data-[disabled]:cursor-default data-[disabled]:opacity-40 data-[highlighted]:bg-panel-2 data-[highlighted]:text-fg";

interface PinRowProps {
  p: Pin;
  parent: string;
  active: boolean;
  dot?: { cls: string; status: string };
  canUp: boolean;
  canDown: boolean;
  onMove: (dir: -1 | 1) => void;
  onUnpin: () => void;
}

function PinRow({ p, parent, active, dot, canUp, canDown, onMove, onUnpin }: PinRowProps) {
  const Icon = PIN_ICON[p.kind];
  const target = pinTarget(p);
  const tip = p.sublabel ? `${p.label} · ${p.sublabel}` : p.label;
  // Alt+↑/↓ reorders the focused pin; Delete unpins it.
  const onKeyDown = (e: KeyboardEvent<HTMLAnchorElement>) => {
    if (e.altKey && e.key === "ArrowUp" && canUp) onMove(-1);
    else if (e.altKey && e.key === "ArrowDown" && canDown) onMove(1);
    else if (e.key === "Delete") onUnpin();
    else return;
    e.preventDefault();
    e.stopPropagation();
  };
  return (
    <div className="group/pin relative flex min-w-0 items-center">
      {active && <span className="absolute top-1.5 bottom-1.5 -left-2 w-0.5 rounded bg-accent" />}
      <Link
        to={target.to}
        search={target.search}
        data-navkey
        data-parent={parent}
        title={tip}
        aria-current={active ? "page" : undefined}
        aria-keyshortcuts="Alt+ArrowUp Alt+ArrowDown Delete"
        onKeyDown={onKeyDown}
        className={`flex min-w-0 flex-1 items-center gap-2 rounded-md py-1 pr-12 pl-2 text-[12.5px] transition ${
          active ? "bg-accent-soft font-medium text-accent" : "text-muted hover:bg-panel-2 hover:text-fg"
        }`}
      >
        <span className="relative shrink-0">
          <Icon className="size-3.5" />
          {dot && <span className={`absolute -right-0.5 -bottom-0.5 size-1.5 rounded-full ring-2 ring-sidebar ${dot.cls}`} title={dot.status} aria-hidden />}
        </span>
        <span className="min-w-0 truncate">{p.label}</span>
        {p.sublabel && <span className="min-w-0 shrink truncate text-[11px] text-faint">{p.sublabel}</span>}
      </Link>
      <div className="absolute right-1 flex items-center opacity-0 transition group-focus-within/pin:opacity-100 group-hover/pin:opacity-100 has-[[data-state=open]]:opacity-100">
        <DropdownMenu.Root modal={false}>
          <DropdownMenu.Trigger asChild>
            <button type="button" aria-label={`Options for ${p.label}`} className="grid size-5 place-items-center rounded text-faint hover:bg-panel-2 hover:text-fg focus-visible:outline-2 focus-visible:outline-accent">
              <MoreHorizontal className="size-3.5" />
            </button>
          </DropdownMenu.Trigger>
          <DropdownMenu.Portal>
            <DropdownMenu.Content side="right" align="start" sideOffset={6} className={menuContent}>
              <DropdownMenu.Item className={menuItem} disabled={!canUp} onSelect={() => onMove(-1)}>
                <ArrowUp className="size-3.5" /> Move up
              </DropdownMenu.Item>
              <DropdownMenu.Item className={menuItem} disabled={!canDown} onSelect={() => onMove(1)}>
                <ArrowDown className="size-3.5" /> Move down
              </DropdownMenu.Item>
              <DropdownMenu.Item className={menuItem} onSelect={onUnpin}>
                <PinOff className="size-3.5" /> Unpin
              </DropdownMenu.Item>
            </DropdownMenu.Content>
          </DropdownMenu.Portal>
        </DropdownMenu.Root>
        <button type="button" onClick={onUnpin} aria-label={`Unpin ${p.label}`} title="Unpin" className="grid size-5 place-items-center rounded text-faint hover:bg-panel-2 hover:text-bad focus-visible:outline-2 focus-visible:outline-accent">
          <X className="size-3.5" />
        </button>
      </div>
    </div>
  );
}

/** Collapsed sidebar: one pin icon opening the list. */
export function PinnedFlyout({ isActive }: { isActive: (to: string) => boolean }) {
  const pins = usePins().data;
  if (!pins?.available) return null;
  const items = pins.items;
  const active = items.some((p) => isActive(pinTarget(p).to));
  return (
    <DropdownMenu.Root modal={false}>
      <DropdownMenu.Trigger asChild>
        <button
          type="button"
          title={`Pinned · ${items.length}`}
          aria-label={`Pinned, ${items.length} items`}
          className={`relative flex w-full items-center justify-center rounded-md py-1.5 transition data-[state=open]:bg-panel-2 ${active ? "text-accent" : "text-muted hover:bg-panel-2 hover:text-fg"}`}
        >
          <PinIcon className="size-4" />
        </button>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content side="right" align="start" sideOffset={8} className={`${menuContent} max-w-[280px]`}>
          <DropdownMenu.Label className="px-2 pt-1 pb-1.5 text-[10px] font-semibold tracking-widest text-faint uppercase">Pinned</DropdownMenu.Label>
          {items.length === 0 && <p className="px-2 pb-1.5 text-[11px] text-faint">Pin a project, service, node or cluster from its page.</p>}
          {items.map((p) => {
            const Icon = PIN_ICON[p.kind];
            const t = pinTarget(p);
            return (
              <DropdownMenu.Item key={`${p.kind}:${p.id}`} asChild>
                <Link to={t.to} search={t.search} className={`${menuItem} min-w-0`} title={p.sublabel ? `${p.label} · ${p.sublabel}` : p.label}>
                  <Icon className="size-3.5 shrink-0" />
                  <span className="min-w-0 truncate">{p.label}</span>
                  {p.sublabel && <span className="min-w-0 truncate text-[11px] text-faint">{p.sublabel}</span>}
                </Link>
              </DropdownMenu.Item>
            );
          })}
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}
