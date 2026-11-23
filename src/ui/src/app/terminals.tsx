import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";

import { LoginPicker, type PickNode, rememberLogin } from "../components/terminal/LoginPicker";
import type { RelayMsg } from "../components/terminal/TerminalPane";
import { useStepUp } from "./stepUp";

/**
 * Terminal sessions live at the application-shell level, so they survive
 * navigation. The docked panel (TerminalDock) renders them; a session can be
 * popped out into its own window, which mirrors the live session through a
 * BroadcastChannel while this window keeps the WebSocket.
 */

/**
 * What a log tab follows. One variant per log source; a new source (container,
 * application, Nginx, Forge itself) adds a variant here and a case in `LogPane`.
 */
export type LogSpec = {
  source: "service";
  appId: string;
  serverId: string;
  /** Service name, for the tab label. */
  name: string;
  /** Node name, for the tab label. */
  node: string;
};

/** Identity of a log tab: opening the same source again activates the existing tab. */
export const logSpecKey = (l: LogSpec) => `${l.source}:${l.appId}:${l.serverId}`;

export interface TermSession {
  key: string;
  serverId: string;
  /** SSH login on the node; a container shell runs through the agent and needs none, so this is `""`. */
  login: string;
  hostname: string;
  role: string;
  /** Shown in a pop-out window instead of the dock. */
  detached: boolean;
  /** `node` (default): SSH to the node. `container`: a shell inside a Docker container on it. `logs`: a log viewer. */
  kind?: "node" | "container" | "logs";
  /** `logs` only: what the tab follows. */
  logs?: LogSpec;
  /** `container` only: Docker id or name used in the WebSocket URL. */
  containerId?: string;
  /** `container` only: name shown on the tab. */
  containerName?: string;
}

/** Tab label: `login@host` for SSH, `name @ host` for a container shell, `service (node)` for logs. */
export const sessionLabel = (s: Pick<TermSession, "login" | "hostname" | "kind" | "containerName" | "containerId" | "logs">) =>
  s.kind === "logs" && s.logs
    ? `${s.logs.name} (${s.logs.node})`
    : s.kind === "container"
      ? `${s.containerName || s.containerId || "container"} @ ${s.hostname}`
      : `${s.login}@${s.hostname}`;

/** A shell (SSH or container), as opposed to a log viewer. */
export const isShell = (s: Pick<TermSession, "kind">) => s.kind !== "logs";

export interface TermPrefs {
  themeId: string;
  fontSize: number;
  copyOnSelect: boolean;
  lastLogin: string;
}

interface DockState {
  open: boolean;
  height: number;
  maximized: boolean;
}

interface Ctx {
  sessions: TermSession[];
  active?: string;
  dock: DockState;
  prefs: TermPrefs;
  open(node: { id: string; hostname: string; role: string }, login?: string): void;
  /** A shell inside a running container on `node` (`docker exec`), in the dock. Returns the session key. */
  openContainer(node: { id: string; hostname: string; role: string; display_name?: string | null }, container: { id: string; name?: string }): string;
  /** A log viewer in the dock; an open tab for the same source is activated instead. Returns the session key. */
  openLogs(spec: LogSpec): string;
  close(key: string): void;
  closeAll(): void;
  activate(key: string): void;
  detach(key: string): boolean;
  attach(key: string): void;
  setDock(d: Partial<DockState>): void;
  setPrefs(p: Partial<TermPrefs>): void;
}

const TerminalsContext = createContext<Ctx | null>(null);

const PREFS_KEY = "forge.terminal";
const DOCK_KEY = "forge.terminal.dock";
const DEFAULT_PREFS: TermPrefs = { themeId: "ocean", fontSize: 14, copyOnSelect: true, lastLogin: "" };

function load<T>(key: string, fallback: T): T {
  try {
    return { ...fallback, ...(JSON.parse(localStorage.getItem(key) ?? "{}") as Partial<T>) };
  } catch {
    return fallback;
  }
}
function save(key: string, v: unknown) {
  try {
    localStorage.setItem(key, JSON.stringify(v));
  } catch {
    /* not remembered */
  }
}

export const loadTermPrefs = () => load(PREFS_KEY, DEFAULT_PREFS);

export function popoutUrl(s: TermSession) {
  const q = new URLSearchParams({ key: s.key, node: s.serverId, login: s.login, host: s.hostname, role: s.role });
  if (s.kind === "container" && s.containerId) {
    q.set("container", s.containerId);
    q.set("cname", s.containerName ?? "");
  }
  return `/popout/terminal?${q}`;
}

export function TerminalsProvider({ children }: { children: ReactNode }) {
  const [sessions, setSessions] = useState<TermSession[]>([]);
  const [active, setActive] = useState<string>();
  const [dock, setDockState] = useState<DockState>(() => ({ ...load(DOCK_KEY, { open: true, height: 340, maximized: false }), open: true }));
  const [prefs, setPrefsState] = useState(() => load(PREFS_KEY, DEFAULT_PREFS));
  const { ensureMfa } = useStepUp();
  const windows = useRef<Record<string, Window | null>>({});
  const seq = useRef(0);

  const setDock = useCallback((d: Partial<DockState>) => {
    setDockState((cur) => {
      const next = { ...cur, ...d };
      save(DOCK_KEY, { height: next.height });
      return next;
    });
  }, []);
  const setPrefs = useCallback((p: Partial<TermPrefs>) => {
    setPrefsState((cur) => {
      const next = { ...cur, ...p };
      save(PREFS_KEY, next);
      return next;
    });
  }, []);

  const start = useCallback(
    (node: { id: string; hostname: string; role: string; display_name?: string | null }, login?: string) => {
      const user = (login ?? prefs.lastLogin).trim();
      if (!user) return;
      rememberLogin(node.id, user);
      seq.current += 1;
      const key = `${node.id.slice(0, 8)}-${Date.now().toString(36)}-${seq.current}`;
      setSessions((s) => [...s, { key, serverId: node.id, login: user, hostname: node.display_name || node.hostname, role: node.role, detached: false }]);
      setActive(key);
      setDock({ open: true });
      setPrefs({ lastLogin: user });
    },
    [prefs.lastLogin, setDock, setPrefs],
  );

  // Container shells run through the node's agent (exec): no login to pick, and
  // no MFA step-up (`container.exec` is not MFA-guarded; the server decides).
  const openContainer = useCallback(
    (node: { id: string; hostname: string; role: string; display_name?: string | null }, container: { id: string; name?: string }) => {
      seq.current += 1;
      const key = `c-${node.id.slice(0, 8)}-${Date.now().toString(36)}-${seq.current}`;
      setSessions((s) => [
        ...s,
        {
          key,
          serverId: node.id,
          login: "",
          hostname: node.display_name || node.hostname,
          role: node.role,
          detached: false,
          kind: "container",
          containerId: container.id,
          containerName: container.name || container.id.slice(0, 12),
        },
      ]);
      setActive(key);
      setDock({ open: true });
      return key;
    },
    [setDock],
  );

  // Log tabs need no login or step-up; reading logs is authorized per request by the server.
  const sessionsRef = useRef<TermSession[]>([]);
  sessionsRef.current = sessions;
  const openLogs = useCallback(
    (spec: LogSpec) => {
      const id = logSpecKey(spec);
      const existing = sessionsRef.current.find((s) => s.kind === "logs" && s.logs && logSpecKey(s.logs) === id);
      let key = existing?.key;
      if (!key) {
        seq.current += 1;
        key = `l-${spec.serverId.slice(0, 8)}-${Date.now().toString(36)}-${seq.current}`;
        const k = key;
        setSessions((s) => [...s, { key: k, serverId: spec.serverId, login: "", hostname: spec.node, role: "", detached: false, kind: "logs", logs: spec }]);
      }
      setActive(key);
      setDock({ open: true });
      return key;
    },
    [setDock],
  );

  // Without a login, ask which account to use (LoginPicker) first.
  const [picking, setPicking] = useState<(PickNode & { role: string }) | null>(null);
  const open = useCallback(
    (node: { id: string; hostname: string; role: string; display_name?: string | null }, login?: string) => {
      if (!login?.trim()) {
        setPicking(node);
        return;
      }
      void (async () => {
        if (!(await ensureMfa(`Opening a terminal on ${node.display_name || node.hostname} needs an MFA-verified sign-in.`))) return;
        start(node, login);
      })();
    },
    [ensureMfa, start],
  );

  const close = useCallback((key: string) => {
    const w = windows.current[key];
    if (w && !w.closed) {
      new BroadcastChannel(`forge-term:${key}`).postMessage({ type: "gone", reason: "session closed from the main window" } satisfies RelayMsg);
      w.close();
    }
    delete windows.current[key];
    setSessions((s) => {
      const rest = s.filter((x) => x.key !== key);
      setActive((a) => (a === key ? rest.filter((x) => !x.detached).at(-1)?.key ?? rest.at(-1)?.key : a));
      return rest;
    });
  }, []);

  const closeAll = useCallback(() => {
    for (const s of sessions) close(s.key);
  }, [sessions, close]);

  const detach = useCallback(
    (key: string) => {
      const s = sessions.find((x) => x.key === key);
      if (!s || s.kind === "logs") return false;
      const w = window.open(popoutUrl(s), `forge-term-${key}`, "popup=yes,width=960,height=600");
      if (!w) return false;
      windows.current[key] = w;
      setSessions((all) => all.map((x) => (x.key === key ? { ...x, detached: true } : x)));
      return true;
    },
    [sessions],
  );

  const attach = useCallback((key: string) => {
    const w = windows.current[key];
    delete windows.current[key];
    if (w && !w.closed) w.close();
    setSessions((all) => all.map((x) => (x.key === key ? { ...x, detached: false } : x)));
    setActive(key);
    setDockState((d) => ({ ...d, open: true }));
  }, []);

  // A pop-out that disappears without saying goodbye comes back to the dock.
  useEffect(() => {
    if (!sessions.some((s) => s.detached)) return;
    const t = setInterval(() => {
      for (const s of sessions) {
        const w = windows.current[s.key];
        if (s.detached && w?.closed) attach(s.key);
      }
    }, 1500);
    return () => clearInterval(t);
  }, [sessions, attach]);

  const value = useMemo<Ctx>(
    () => ({ sessions, active, dock, prefs, open, openContainer, openLogs, close, closeAll, activate: setActive, detach, attach, setDock, setPrefs }),
    [sessions, active, dock, prefs, open, openContainer, openLogs, close, closeAll, detach, attach, setDock, setPrefs],
  );
  return (
    <TerminalsContext.Provider value={value}>
      {children}
      <LoginPicker
        node={picking}
        onClose={() => setPicking(null)}
        onPick={(login) => {
          const n = picking;
          setPicking(null);
          if (n) open(n, login);
        }}
      />
    </TerminalsContext.Provider>
  );
}

export function useTerminals(): Ctx {
  const c = useContext(TerminalsContext);
  if (!c) throw new Error("useTerminals outside TerminalsProvider");
  return c;
}
