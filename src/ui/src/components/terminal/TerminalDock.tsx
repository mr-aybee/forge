import { Link } from "@tanstack/react-router";
import {
  ArrowDown,
  ArrowUp,
  ChevronDown,
  ChevronUp,
  ClipboardPaste,
  Container,
  Copy,
  Download,
  Eraser,
  ExternalLink,
  Maximize2,
  Minimize2,
  PanelBottomClose,
  Plus,
  RotateCw,
  ScrollText,
  Search,
  Settings2,
  Server,
  SquareTerminal,
  Undo2,
  X,
} from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";

import { type ContainerSummary, useContainerQuery, useMe, useServerLogins, useServers } from "../../api/hooks";
import { usePrefs } from "../../app/prefs";
import { sessionLabel, useTerminals, type TermSession } from "../../app/terminals";
import { errorText, useToast } from "../../app/toast";
import LogPane from "../LogPane";
import { Badge, Button, Field, IconButton, Input, Modal, Select } from "../ui";
import { TerminalPane, relayName, type PaneStatus, type TerminalHandle } from "./TerminalPane";
import { recentLogins } from "./LoginPicker";
import { TERM_THEMES } from "./themes";

export const stateDot: Record<string, string> = {
  connecting: "bg-warn animate-pulse",
  ready: "bg-ok",
  closed: "bg-faint",
  error: "bg-bad",
};

const MIN_H = 160;

/**
 * Bottom-docked, resizable terminal workspace. Mounted once in the shell, so
 * sessions survive navigation; collapsing only hides the panes.
 */
export default function TerminalDock({ onHide }: { onHide(): void }) {
  const t = useTerminals();
  const toast = useToast();
  const [status, setStatus] = useState<Record<string, PaneStatus>>({});
  const [findOpen, setFindOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [newOpen, setNewOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const handles = useRef<Record<string, TerminalHandle | null>>({});
  const drag = useRef<{ y: number; h: number } | null>(null);

  const cur = t.sessions.find((s) => s.key === t.active);
  const isLog = cur?.kind === "logs";
  const current = cur && !cur.detached ? handles.current[cur.key] : null;
  const st = cur ? status[cur.key] : undefined;
  const expanded = t.dock.open;
  const height = t.dock.maximized ? "calc(100vh - 7rem)" : `${t.dock.height}px`;

  const onStatus = useCallback((key: string, s: PaneStatus) => setStatus((m) => ({ ...m, [key]: { ...s } })), []);

  // Drag the top edge to resize.
  useEffect(() => {
    const move = (e: PointerEvent) => {
      if (!drag.current) return;
      const max = window.innerHeight * 0.85;
      t.setDock({ height: Math.round(Math.min(max, Math.max(MIN_H, drag.current.h + drag.current.y - e.clientY))), maximized: false });
    };
    const up = () => {
      drag.current = null;
      document.body.style.userSelect = "";
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    return () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
  }, [t]);

  // Ctrl+` toggles the panel; Ctrl+Shift+T opens a session.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.ctrlKey && !e.shiftKey && e.code === "Backquote") {
        e.preventDefault();
        t.setDock({ open: !t.dock.open });
      } else if (e.ctrlKey && e.shiftKey && e.code === "KeyT") {
        e.preventDefault();
        setNewOpen(true);
      } else if (e.ctrlKey && e.shiftKey && e.code === "KeyF" && current) {
        e.preventDefault();
        setFindOpen(true);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [t, current]);

  const saveTranscript = () => {
    if (!current || !cur) return;
    const blob = new Blob([current.transcript()], { type: "text/plain" });
    const a = document.createElement("a");
    a.href = URL.createObjectURL(blob);
    a.download = `${cur.kind === "container" ? cur.containerName || "container" : cur.hostname}-${new Date().toISOString().replace(/[:.]/g, "-")}.log`;
    a.click();
    URL.revokeObjectURL(a.href);
  };

  const popOut = (s: TermSession) => {
    if (!t.detach(s.key)) toast("The browser blocked the pop-up window. Allow pop-ups for Forge and try again.", "error");
  };

  return (
    <section
      aria-label="Terminal sessions"
      className="relative flex shrink-0 flex-col border-t border-line bg-panel shadow-[0_-4px_16px_rgba(0,0,0,0.12)]"
      style={expanded ? { height } : undefined}
    >
      {expanded && (
        <div
          role="separator"
          aria-orientation="horizontal"
          aria-label="Resize terminal panel"
          className="absolute -top-1 right-0 left-0 z-10 h-2 cursor-row-resize hover:bg-accent/30"
          onPointerDown={(e) => {
            drag.current = { y: e.clientY, h: (e.currentTarget.parentElement?.offsetHeight ?? t.dock.height) };
            document.body.style.userSelect = "none";
          }}
          onDoubleClick={() => t.setDock({ maximized: !t.dock.maximized })}
        />
      )}

      {/* Tab strip */}
      <div className="flex h-9 shrink-0 items-center gap-1 border-b border-line bg-panel-2/60 px-2">
        <button className="flex items-center gap-1.5 px-1 text-xs font-semibold tracking-wide text-muted uppercase hover:text-fg" onClick={() => t.setDock({ open: !expanded })}>
          <SquareTerminal className="size-4 text-accent" /> Terminal
        </button>
        <div className="flex min-w-0 flex-1 items-center gap-1 overflow-x-auto">
          {t.sessions.map((s) => (
            <div
              key={s.key}
              className={`group flex h-7 shrink-0 items-center gap-1.5 rounded-md border px-2 text-[12px] ${
                s.key === t.active && expanded ? "border-line bg-panel text-fg shadow-sm" : "border-transparent text-muted hover:bg-panel"
              }`}
            >
              <button
                className="flex items-center gap-1.5"
                title={`${sessionLabel(s)} — ${status[s.key]?.state ?? "connecting"}`}
                onClick={() => {
                  t.activate(s.key);
                  t.setDock({ open: true });
                }}
                onAuxClick={(e) => e.button === 1 && t.close(s.key)}
              >
                <span className={`size-2 rounded-full ${stateDot[status[s.key]?.state ?? "connecting"]}`} />
                {s.kind === "logs" && <ScrollText className="size-3 shrink-0 text-accent" aria-label="logs" />}
                {s.kind === "container" && <Container className="size-3 shrink-0 text-accent" aria-label="container shell" />}
                <span className="max-w-[220px] truncate font-mono">{sessionLabel(s)}</span>
                {s.role === "control" && !s.kind && <Badge tone="accent">control</Badge>}
                {s.detached && <ExternalLink className="size-3 text-accent" aria-label="in a separate window" />}
              </button>
              <button aria-label={`Close ${sessionLabel(s)}`} className="rounded text-faint opacity-60 hover:text-fg group-hover:opacity-100" onClick={() => t.close(s.key)}>
                <X className="size-3.5" />
              </button>
            </div>
          ))}
          <IconButton label="New session (Ctrl+Shift+T)" onClick={() => setNewOpen(true)}>
            <Plus className="size-4" />
          </IconButton>
        </div>
        <div className="flex items-center gap-0.5">
          {expanded && (
            <>
              {!isLog && (
                <>
              <IconButton label="Find (Ctrl+Shift+F)" disabled={!current} onClick={() => setFindOpen((o) => !o)}>
                <Search className="size-4" />
              </IconButton>
              <IconButton label="Copy selection" disabled={!current} onClick={() => void current?.copySelection()}>
                <Copy className="size-4" />
              </IconButton>
              <IconButton label="Paste" disabled={!current} onClick={() => void current?.paste()}>
                <ClipboardPaste className="size-4" />
              </IconButton>
              <IconButton label="Clear scrollback" disabled={!current} onClick={() => current?.clear()}>
                <Eraser className="size-4" />
              </IconButton>
              <IconButton label="Save transcript" disabled={!current} onClick={saveTranscript}>
                <Download className="size-4" />
              </IconButton>
              <IconButton label="Reconnect" disabled={!current} onClick={() => current?.reconnect()}>
                <RotateCw className="size-4" />
              </IconButton>
              <IconButton label="Open in a separate window" disabled={!cur || cur.detached} onClick={() => cur && popOut(cur)}>
                <ExternalLink className="size-4" />
              </IconButton>
              <IconButton label="Terminal settings" onClick={() => setSettingsOpen(true)}>
                <Settings2 className="size-4" />
              </IconButton>
                </>
              )}
              <IconButton label={t.dock.maximized ? "Restore panel size" : "Maximize panel"} onClick={() => t.setDock({ maximized: !t.dock.maximized })}>
                {t.dock.maximized ? <Minimize2 className="size-4" /> : <Maximize2 className="size-4" />}
              </IconButton>
            </>
          )}
          <IconButton label={expanded ? "Collapse (Ctrl+`)" : "Expand (Ctrl+`)"} onClick={() => t.setDock({ open: !expanded })}>
            {expanded ? <ChevronDown className="size-4" /> : <ChevronUp className="size-4" />}
          </IconButton>
          <IconButton
            label="Close all sessions and hide"
            onClick={() => {
              t.closeAll();
              onHide();
            }}
          >
            <PanelBottomClose className="size-4" />
          </IconButton>
        </div>
      </div>

      {expanded && findOpen && current && !isLog && (
        <form
          className="flex items-center gap-2 border-b border-line px-3 py-1"
          onSubmit={(e) => {
            e.preventDefault();
            current.findNext(query);
          }}
        >
          <Search className="size-4 text-faint" />
          <input
            autoFocus
            data-bare className="min-w-0 flex-1 bg-transparent text-[13px] outline-none"
            placeholder="Find in scrollback…"
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              current.findNext(e.target.value);
            }}
            onKeyDown={(e) => {
              if (e.key === "Escape") {
                current.clearSearch();
                setFindOpen(false);
                current.focus();
              }
            }}
          />
          <IconButton label="Previous match" type="button" onClick={() => current.findPrevious(query)}>
            <ArrowUp className="size-4" />
          </IconButton>
          <IconButton label="Next match" type="submit">
            <ArrowDown className="size-4" />
          </IconButton>
          <IconButton
            label="Close find"
            type="button"
            onClick={() => {
              current.clearSearch();
              setFindOpen(false);
            }}
          >
            <X className="size-4" />
          </IconButton>
        </form>
      )}

      {/* Panes stay mounted while collapsed or detached so sessions keep running. */}
      <div className={`relative min-h-0 flex-1 ${expanded ? "flex" : "hidden"}`}>
        {t.sessions.map((s) => {
          const shown = expanded && s.key === t.active && !s.detached;
          return (
            <div key={s.key} className={`min-w-0 flex-1 ${shown ? "flex" : "hidden"}`}>
              {s.kind === "logs" && s.logs ? (
                <LogPane spec={s.logs} visible={shown} onStatus={(x) => onStatus(s.key, x)} />
              ) : (
              <TerminalPane
                ref={(h) => {
                  handles.current[s.key] = h;
                }}
                serverId={s.serverId}
                login={s.login}
                container={s.kind === "container" ? s.containerId : undefined}
                themeId={t.prefs.themeId}
                fontSize={t.prefs.fontSize}
                copyOnSelect={t.prefs.copyOnSelect}
                visible={shown}
                relay={s.detached ? relayName(s.key) : undefined}
                onRelayRequest={(kind) => (kind === "attach" ? t.attach(s.key) : t.close(s.key))}
                onStatus={(x) => onStatus(s.key, x)}
              />
              )}
            </div>
          );
        })}
        {t.sessions.length === 0 && (
          <div className="grid flex-1 place-items-center p-6 text-center text-[13px] text-muted">
            <div className="space-y-3">
              <p>No open sessions. Sessions stay connected while you move around the console.</p>
              <Button variant="primary" onClick={() => setNewOpen(true)}>
                <Plus className="size-4" /> New session
              </Button>
            </div>
          </div>
        )}
        {cur?.detached && (
          <div className="grid flex-1 place-items-center p-6 text-center text-[13px] text-muted">
            <div className="space-y-3">
              <p>
                <span className="font-mono text-fg">{sessionLabel(cur)}</span> is open in a separate window. The session runs through this window — keep it open.
              </p>
              <div className="flex justify-center gap-2">
                <Button onClick={() => window.open("", `forge-term-${cur.key}`)?.focus()}>
                  <ExternalLink className="size-4" /> Show window
                </Button>
                <Button variant="primary" onClick={() => t.attach(cur.key)}>
                  <Undo2 className="size-4" /> Bring back to the panel
                </Button>
              </div>
            </div>
          </div>
        )}
      </div>

      {expanded && (
        <div className="flex h-6 shrink-0 items-center gap-4 border-t border-line bg-panel-2/60 px-3 font-mono text-[11px] text-faint">
          {cur ? (
            <>
              <span className="flex items-center gap-1.5">
                <span className={`size-2 rounded-full ${stateDot[st?.state ?? "connecting"]}`} />
                {st?.state ?? "connecting"}
                {st?.state === "error" && st.message && <span className="truncate text-bad" title={st.message}>— {st.message}</span>}
              </span>
              {cur.kind === "logs" && cur.logs ? (
                <span className="flex min-w-0 items-center gap-1 truncate">
                  <Link to="/services/$serviceId" params={{ serviceId: cur.logs.appId }} className="hover:text-accent">
                    {cur.logs.name}
                  </Link>
                  <span>→</span>
                  <Link to="/nodes/$nodeId" params={{ nodeId: cur.logs.serverId }} className="hover:text-accent">
                    {cur.logs.node}
                  </Link>
                </span>
              ) : cur.kind === "container" && cur.containerId ? (
                <Link to="/nodes/$nodeId/containers/$containerId" params={{ nodeId: cur.serverId, containerId: cur.containerId }} search={{ tab: "shell" }} className="hover:text-accent">
                  {sessionLabel(cur)}
                </Link>
              ) : (
                <Link to="/nodes/$nodeId" params={{ nodeId: cur.serverId }} className="hover:text-accent">
                  {sessionLabel(cur)}
                </Link>
              )}
              {!isLog && st?.cols && (
                <span>
                  {st.cols}×{st.rows}
                </span>
              )}
              {st?.latencyMs !== undefined && <span className={st.latencyMs > 250 ? "text-warn" : ""}>{st.latencyMs} ms</span>}
              <span className="ml-auto hidden sm:inline">{isLog ? "service logs via the node's agent · Ctrl+` toggle" : cur.kind === "container" ? "exec via the node's agent · audited · Ctrl+` toggle" : "SSH certificate (5 min) · audited · Ctrl+` toggle"}</span>
            </>
          ) : (
            <span>Ctrl+Shift+T new session · Ctrl+` toggle panel</span>
          )}
        </div>
      )}

      <NewSession open={newOpen} onOpenChange={setNewOpen} />
      <TerminalSettings open={settingsOpen} onOpenChange={setSettingsOpen} />
    </section>
  );
}

export function NewSession({ open, onOpenChange, nodeId }: { open: boolean; onOpenChange(o: boolean): void; nodeId?: string }) {
  return (
    <Modal open={open} onOpenChange={onOpenChange} title="New session" description="A shell on a node over SSH, or inside a container on a node.">
      {open && <NewSessionForm nodeId={nodeId} onDone={() => onOpenChange(false)} />}
    </Modal>
  );
}

type Target = "node" | "container";
const LOGIN_RE = /^[A-Za-z0-9_][A-Za-z0-9_.-]{0,31}$/;
const OTHER = "\u0000other";

/** Mounted only while the dialog is open, so its initial state is the reset. */
function NewSessionForm({ nodeId, onDone }: { nodeId?: string; onDone(): void }) {
  const t = useTerminals();
  const me = useMe().data;
  const all = (useServers().data ?? []).filter((n) => n.status !== "revoked");
  const [f, setF] = useState<{ target: Target; node: string; login: string; other: string; container: string }>(() => ({
    target: "node",
    node: nodeId ?? "",
    login: "",
    other: "",
    container: "",
  }));
  const set = (p: Partial<typeof f>) => setF((c) => ({ ...c, ...p }));

  // Containers need the node's agent online; SSH works to any enrolled node.
  const nodes = f.target === "container" ? all.filter((n) => n.status === "online") : all;
  const node = nodes.find((n) => n.id === f.node) ?? nodes[0];

  const logins = useServerLogins(f.target === "node" ? node?.id : undefined);
  const recent = node ? recentLogins(node.id) : [];
  const accounts = [...new Set([...(logins.data?.saved ?? logins.data?.logins ?? []), ...(logins.data?.discovered ?? []), ...recent])];
  // Default account: the last one used on this node, else the first known one.
  const login = f.login || (t.prefs.lastLogin && accounts.includes(t.prefs.lastLogin) ? t.prefs.lastLogin : accounts[0]) || OTHER;
  const typed = login === OTHER ? f.other.trim() : login;

  const list = useContainerQuery<ContainerSummary[]>(node?.id ?? "", "list", f.target === "container" && !!node);
  const running = (list.data?.result ?? []).filter((c) => c.state === "running").sort((a, b) => a.name.localeCompare(b.name));
  const listError = list.error ? errorText(list.error) : list.data && list.data.status !== "succeeded" ? (list.data.error ?? "Docker is not available on this node.") : undefined;
  const container = running.find((c) => c.id === f.container) ?? running[0];

  const ready = !!node && (f.target === "node" ? LOGIN_RE.test(typed) : !!container);

  return (
    <form
      className="space-y-4"
      onSubmit={(e) => {
        e.preventDefault();
        if (!ready || !node) return;
        if (f.target === "node") t.open(node, typed);
        else if (container) t.openContainer(node, { id: container.id, name: container.name });
        onDone();
      }}
    >
      <div className="grid grid-cols-2 gap-1 rounded-lg border border-line bg-panel-2 p-1" role="radiogroup" aria-label="Connect to">
        {(
          [
            ["node", Server, "Node", "SSH as an account on the node"],
            ["container", Container, "Container", "A shell inside a running container"],
          ] as const
        ).map(([id, Icon, label, body]) => (
          <button
            key={id}
            type="button"
            role="radio"
            aria-checked={f.target === id}
            onClick={() => set({ target: id, container: "" })}
            className={`flex items-start gap-2 rounded-md px-3 py-2 text-left transition ${f.target === id ? "bg-panel shadow-sm ring-1 ring-accent/40" : "text-muted hover:text-fg"}`}
          >
            <Icon className={`mt-0.5 size-4 shrink-0 ${f.target === id ? "text-accent" : ""}`} />
            <span className="min-w-0">
              <span className="block text-[13px] font-medium">{label}</span>
              <span className="block text-[11px] text-faint">{body}</span>
            </span>
          </button>
        ))}
      </div>

      <Field label="Node" hint={f.target === "container" ? "Only nodes whose agent is online." : undefined}>
        <Select value={node?.id ?? ""} onChange={(e) => set({ node: e.target.value, login: "", container: "" })} required>
          {nodes.length === 0 && <option value="">No nodes available</option>}
          {nodes.map((n) => (
            <option key={n.id} value={n.id}>
              {n.display_name || n.hostname} ({n.role}) — {n.status}
            </option>
          ))}
        </Select>
      </Field>

      {f.target === "node" ? (
        <Field label="Account" hint="Accounts saved for this node, reported by it, or used recently. Root login stays whatever the node's sshd allows.">
          <Select value={login} onChange={(e) => set({ login: e.target.value })} disabled={!node || logins.isLoading}>
            {logins.isLoading && <option value={login}>Loading accounts…</option>}
            {!logins.isLoading &&
              accounts.map((a) => (
                <option key={a} value={a}>
                  {a}
                </option>
              ))}
            <option value={OTHER}>Other account…</option>
          </Select>
          {logins.error && <p className="mt-1 text-xs text-bad">Could not list accounts: {errorText(logins.error)}</p>}
          {login === OTHER && (
            <Input
              className="mt-2 font-mono"
              aria-label="Account name"
              placeholder="e.g. deploy"
              value={f.other}
              onChange={(e) => set({ other: e.target.value })}
              pattern="[A-Za-z0-9_][A-Za-z0-9_.\-]{0,31}"
              required
              autoFocus
            />
          )}
        </Field>
      ) : (
        <>
          <Field label="Container">
            <Select value={container?.id ?? ""} onChange={(e) => set({ container: e.target.value })} disabled={!node || list.isLoading || !running.length}>
              {list.isLoading ? (
                <option value="">Loading containers…</option>
              ) : running.length === 0 ? (
                <option value="">{listError ? "Containers unavailable" : "No running containers"}</option>
              ) : (
                running.map((c) => (
                  <option key={c.id} value={c.id}>
                    {c.name} — {c.image}
                  </option>
                ))
              )}
            </Select>
            {listError && <p className="mt-1 text-xs text-bad">{listError}</p>}
          </Field>
          <Field label="Account" hint="Container shells run as the container's own user (the image's USER, often root), the same as docker exec.">
            <Input value="Container default user" disabled readOnly />
          </Field>
        </>
      )}

      {f.target === "node" && me && !me.mfa && <p className="text-xs text-warn">SSH sessions require an MFA-verified sign-in.</p>}
      <div className="flex justify-end gap-2">
        <Button type="button" variant="ghost" onClick={onDone}>
          Cancel
        </Button>
        <Button variant="primary" disabled={!ready}>
          <SquareTerminal className="size-4" /> Connect
        </Button>
      </div>
    </form>
  );
}

function TerminalSettings({ open, onOpenChange }: { open: boolean; onOpenChange(o: boolean): void }) {
  const { prefs, setPrefs } = useTerminals();
  const { consoleTheme, setConsoleTheme } = usePrefs();
  return (
    <Modal open={open} onOpenChange={onOpenChange} title="Terminal settings" description="Saved in this browser.">
      <div className="space-y-4">
        <Field label="Log consoles" hint="Matches the app theme when the page opens; a change here lasts until you reload. Terminals use the colour scheme below.">
          <div className="grid grid-cols-2 gap-2" role="radiogroup" aria-label="Log console colours">
            {(
              [
                ["dark", "Dark", "Light text on a dark background"],
                ["light", "Light", "Dark text on a light background"],
              ] as const
            ).map(([id, label, body]) => (
              <button
                key={id}
                type="button"
                role="radio"
                aria-checked={consoleTheme === id}
                onClick={() => setConsoleTheme(id)}
                className={`flex items-center gap-2 rounded-lg border p-2 text-left text-xs ${consoleTheme === id ? "border-accent ring-1 ring-accent/40" : "border-line hover:border-accent/50"}`}
              >
                <span className={`grid h-7 w-10 shrink-0 place-items-center rounded font-mono text-[10px] ${id === "dark" ? "bg-[#06121f] text-[#d6e6f2]" : "border border-line bg-white text-[#0f172a]"}`}>abc</span>
                <span className="min-w-0">
                  <span className="block font-medium">{label}</span>
                  <span className="block text-faint">{body}</span>
                </span>
              </button>
            ))}
          </div>
        </Field>
        <Field label="Colour scheme">
          <div className="grid grid-cols-2 gap-2">
            {TERM_THEMES.map((th) => (
              <button
                key={th.id}
                onClick={() => setPrefs({ themeId: th.id })}
                className={`rounded-lg border p-2 text-left text-xs ${prefs.themeId === th.id ? "border-accent ring-1 ring-accent/40" : "border-line hover:border-accent/50"}`}
              >
                <div className="mb-1.5 rounded px-2 py-1.5 font-mono" style={{ background: th.theme.background, color: th.theme.foreground }}>
                  <span style={{ color: th.theme.green }}>forgeops</span>@node:~$ <span style={{ color: th.theme.cyan }}>ls</span>
                </div>
                {th.label}
              </button>
            ))}
          </div>
        </Field>
        <Field label={`Font size: ${prefs.fontSize}px`}>
          <input type="range" min={10} max={24} value={prefs.fontSize} onChange={(e) => setPrefs({ fontSize: Number(e.target.value) })} className="w-full accent-[var(--color-accent)]" />
        </Field>
        <label className="flex items-center gap-2 text-[13px]">
          <input type="checkbox" checked={prefs.copyOnSelect} onChange={(e) => setPrefs({ copyOnSelect: e.target.checked })} />
          Copy text to the clipboard when selected (Ctrl+C with a selection and right-click also copy)
        </label>
      </div>
    </Modal>
  );
}
