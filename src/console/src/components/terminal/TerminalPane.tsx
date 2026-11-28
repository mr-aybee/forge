/** A short note over the terminal ("Copied"). */
function flash(text: string) {
  const el = document.createElement("div");
  el.textContent = text;
  el.className = "pointer-events-none fixed bottom-6 left-1/2 z-[100] -translate-x-1/2 rounded-md bg-black/80 px-3 py-1.5 text-xs text-white shadow";
  document.body.appendChild(el);
  setTimeout(() => el.remove(), 1200);
}

import "@xterm/xterm/css/xterm.css";
import { COPY_BLOCKED, copyText } from "../../lib/clipboard";

import { ClipboardAddon } from "@xterm/addon-clipboard";
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import { SerializeAddon } from "@xterm/addon-serialize";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { WebglAddon } from "@xterm/addon-webgl";
import { Terminal } from "@xterm/xterm";
import { ClipboardCopy, ClipboardPaste, Eraser, TextSelect } from "lucide-react";
import { forwardRef, useCallback, useEffect, useImperativeHandle, useRef, useState } from "react";
import { PointerMenu, type PointerMenuState } from "../ui";

import { ApiError, csrfToken, get } from "../../api/client";
import { themeById } from "./themes";

export type ConnState = "connecting" | "ready" | "closed" | "error";

export interface PaneStatus {
  state: ConnState;
  message?: string;
  latencyMs?: number;
  cols?: number;
  rows?: number;
}

export interface TerminalHandle {
  focus(): void;
  reconnect(): void;
  findNext(q: string): boolean;
  findPrevious(q: string): boolean;
  clearSearch(): void;
  clear(): void;
  copySelection(): Promise<void>;
  paste(): Promise<void>;
  /** Plain-text dump of the scrollback (for "Save transcript"). */
  transcript(): string;
}

/**
 * Messages on the per-session BroadcastChannel between the window that owns
 * the WebSocket and a pop-out window mirroring it.
 */
export type RelayMsg =
  | { type: "hello" }
  | { type: "snapshot"; data: string; status: PaneStatus }
  | { type: "out"; data: Uint8Array }
  | { type: "status"; status: PaneStatus }
  | { type: "in"; data: string }
  | { type: "resize"; cols: number; rows: number }
  | { type: "reconnect" }
  /** Pop-out → owner: put the session back in the dock (window closing). */
  | { type: "attach" }
  /** Pop-out → owner: end the session. */
  | { type: "terminate" }
  /** Owner → pop-out: the owner window is going away / session closed. */
  | { type: "gone"; reason: string };

export const relayName = (key: string) => `forge-term:${key}`;

interface Props {
  serverId: string;
  login: string;
  /**
   * When set, the pane opens a shell inside this Docker container (id or name)
   * on the node (`/servers/{id}/containers/{container}/shell`) instead of SSH.
   */
  container?: string;
  themeId: string;
  fontSize: number;
  copyOnSelect: boolean;
  /** Whether this pane is visible; hidden panes skip fitting. */
  visible: boolean;
  onStatus(s: PaneStatus): void;
  /**
   * Owner mode: when set, this pane also serves a pop-out window on the named
   * BroadcastChannel (output is mirrored, input and resizes are accepted).
   */
  relay?: string;
  /** Mirror mode: no WebSocket; the session is owned by another window. */
  mirrorOf?: string;
  /** Owner mode: the pop-out asked to come back / to end the session. */
  onRelayRequest?(kind: "attach" | "terminate"): void;
}

const encoder = new TextEncoder();

/**
 * One SSH session in an xterm.js terminal over the Forge WebSocket.
 * Keystrokes go out as binary frames; output arrives as binary frames; JSON
 * text frames carry control messages (auth, resize, ping and status).
 */
export const TerminalPane = forwardRef<TerminalHandle, Props>(function TerminalPane(
  { serverId, login, container, themeId, fontSize, copyOnSelect, visible, onStatus, relay, mirrorOf, onRelayRequest },
  ref,
) {
  // Same handshake and frames for both; only the endpoint differs.
  const endpoint = container ? `/api/v1/servers/${serverId}/containers/${encodeURIComponent(container)}/shell` : `/api/v1/servers/${serverId}/terminal`;
  const host = useRef<HTMLDivElement>(null);
  const term = useRef<Terminal | null>(null);
  const [menu, setMenu] = useState<PointerMenuState | null>(null);
  const openMenu = useRef<((x: number, y: number) => void) | null>(null);
  openMenu.current = (x, y) => {
    const t = term.current;
    if (!t) return;
    const canRead = !!navigator.clipboard?.readText && window.isSecureContext;
    setMenu({
      x,
      y,
      label: "Terminal",
      actions: [
        {
          label: "Copy",
          icon: ClipboardCopy,
          disabled: !t.hasSelection(),
          hint: t.hasSelection() ? "Ctrl+C" : "Select text first",
          onSelect: () => {
            void copyText(t.getSelection()).then((ok) => flash(ok ? "Copied" : COPY_BLOCKED));
            t.clearSelection();
            t.focus();
          },
        },
        {
          label: "Paste",
          icon: ClipboardPaste,
          disabled: !canRead,
          hint: canRead ? "Ctrl+V" : "Use Ctrl+V (the browser blocks menu paste over plain HTTP)",
          onSelect: () => {
            void navigator.clipboard
              .readText()
              .then((txt) => txt && t.paste(txt))
              .catch(() => flash("Paste with Ctrl+V"))
              .finally(() => t.focus());
          },
        },
        { label: "Select all", icon: TextSelect, hint: "Ctrl+Shift+A", onSelect: () => t.selectAll() },
        { label: "Clear screen", icon: Eraser, onSelect: () => { t.clear(); t.focus(); } },
      ],
    });
  };
  const fit = useRef<FitAddon | null>(null);
  const search = useRef<SearchAddon | null>(null);
  const serializer = useRef<SerializeAddon | null>(null);
  const socket = useRef<WebSocket | null>(null);
  const relayChan = useRef<BroadcastChannel | null>(null);
  const mirrorChan = useRef<BroadcastChannel | null>(null);
  const statusRef = useRef(onStatus);
  statusRef.current = onStatus;
  const requestRef = useRef(onRelayRequest);
  requestRef.current = onRelayRequest;
  const status = useRef<PaneStatus>({ state: "connecting" });
  const report = useCallback((s: Partial<PaneStatus>) => {
    status.current = { ...status.current, ...s };
    statusRef.current(status.current);
    relayChan.current?.postMessage({ type: "status", status: status.current } satisfies RelayMsg);
  }, []);

  const sendControl = (msg: object) => {
    const ws = socket.current;
    if (ws?.readyState === WebSocket.OPEN) ws.send(JSON.stringify(msg));
  };
  const sendInput = (d: string) => {
    const ws = socket.current;
    if (ws?.readyState === WebSocket.OPEN && status.current.state === "ready") ws.send(encoder.encode(d));
  };

  const open = useCallback((qs: URLSearchParams) => {
    const t = term.current;
    if (!t) return;
    const proto = location.protocol === "https:" ? "wss" : "ws";
    const ws = new WebSocket(`${proto}://${location.host}${endpoint}?${qs}`);
    ws.binaryType = "arraybuffer";
    socket.current = ws;
    ws.onopen = () => ws.send(JSON.stringify({ type: "auth", csrf: csrfToken() ?? "" }));
    ws.onmessage = (m) => {
      if (m.data instanceof ArrayBuffer) {
        const bytes = new Uint8Array(m.data);
        t.write(bytes);
        relayChan.current?.postMessage({ type: "out", data: bytes } satisfies RelayMsg);
        return;
      }
      const msg = JSON.parse(m.data as string) as { type: string; message?: string; reason?: string; t?: number };
      const note = (line: string) => {
        t.writeln(line);
        relayChan.current?.postMessage({ type: "out", data: encoder.encode(`${line}\r\n`) } satisfies RelayMsg);
      };
      if (msg.type === "ready") {
        report({ state: "ready", message: undefined, cols: t.cols, rows: t.rows });
        if (!relayChan.current) t.focus();
      } else if (msg.type === "pong" && msg.t) {
        report({ latencyMs: Math.round(performance.now() - msg.t) });
      } else if (msg.type === "error") {
        note(`\r\n\x1b[31m✖ ${msg.message ?? "connection failed"}\x1b[0m`);
        report({ state: "error", message: msg.message });
      } else if (msg.type === "closed") {
        note(`\r\n\x1b[2m— session ended (${(msg.reason ?? "closed").replace(/_/g, " ")})\x1b[0m`);
        report({ state: status.current.state === "error" ? "error" : "closed", message: msg.reason });
      }
    };
    ws.onclose = (e) => {
      if (socket.current !== ws) return;
      if (status.current.state === "connecting") {
        const why = e.code === 1006 ? "refused by the server (is this console's address in Settings > Server > Allowed hosts?)" : `closed (${e.code})`;
        t.writeln(`\r\n\x1b[31m✖ connection ${why}\x1b[0m`);
        report({ state: "error", message: why });
      } else if (status.current.state === "ready") {
        report({ state: "closed" });
      }
    };
  }, [endpoint, report]);

  const connect = useCallback(() => {
    const t = term.current;
    if (!t) return;
    if (mirrorOf) {
      mirrorChan.current?.postMessage({ type: "reconnect" } satisfies RelayMsg);
      return;
    }
    socket.current?.close();
    report({ state: "connecting", message: undefined, latencyMs: undefined });
    const target = container ? `container ${container.slice(0, 24)} on ${serverId.slice(0, 8)}` : `${login}@${serverId.slice(0, 8)}`;
    t.writeln(`\x1b[2m— connecting to ${target} …\x1b[0m`);
    const qs = new URLSearchParams({ ...(container ? {} : { login }), cols: String(t.cols), rows: String(t.rows) });
    // Browsers hide why an upgrade was refused; ask first so the real reason
    // (MFA, permission, missing SSH endpoint…) shows, and step-up can prompt.
    // A server without a check route for the container shell answers 404/405:
    // then the upgrade is tried directly.
    get(`${endpoint}/check?${qs}`).then(
      () => open(qs),
      (e: unknown) => {
        if (container && e instanceof ApiError && (e.status === 404 || e.status === 405)) {
          open(qs);
          return;
        }
        const why = e instanceof ApiError ? e.message : String(e);
        t.writeln(`\r\n\x1b[31m✖ ${why}\x1b[0m`);
        report({ state: "error", message: why });
      },
    );
  }, [login, serverId, container, endpoint, report, mirrorOf, open]);

  // Create the terminal once.
  useEffect(() => {
    const el = host.current;
    if (!el) return;
    const t = new Terminal({
      allowProposedApi: true,
      cursorBlink: true,
      cursorStyle: "bar",
      fontFamily: '"JetBrains Mono", "Cascadia Code", Menlo, Consolas, monospace',
      fontSize,
      lineHeight: 1.15,
      scrollback: 10_000,
      macOptionIsMeta: true,
      rightClickSelectsWord: false,
      theme: themeById(themeId).theme,
    });
    const f = new FitAddon();
    const s = new SearchAddon();
    const ser = new SerializeAddon();
    t.loadAddon(f);
    t.loadAddon(s);
    t.loadAddon(ser);
    t.loadAddon(new WebLinksAddon());
    t.loadAddon(new ClipboardAddon());
    const u = new Unicode11Addon();
    t.loadAddon(u);
    t.unicode.activeVersion = "11";
    t.open(el);
    try {
      const gl = new WebglAddon();
      gl.onContextLoss(() => gl.dispose()); // falls back to the DOM renderer
      t.loadAddon(gl);
    } catch {
      /* WebGL unavailable: DOM renderer */
    }
    // Clipboard like Windows Terminal: Ctrl+C copies when text is selected
    // (otherwise it is SIGINT), Ctrl+Shift+C always copies. Ctrl+V and
    // Ctrl+Shift+V (and Shift+Insert) use the browser's own paste event, which
    // xterm handles; that also works on plain-HTTP consoles where the
    // Clipboard API is unavailable. Ctrl+A selects all only with Shift
    // (plain Ctrl+A is the shell's start-of-line).
    t.attachCustomKeyEventHandler((e) => {
      if (e.type !== "keydown") return true;
      if (e.shiftKey && e.code === "Insert") return false;
      if (!e.ctrlKey && !e.metaKey) return true;
      if (e.code === "KeyC" && (e.shiftKey || t.hasSelection())) {
        if (t.hasSelection()) {
          void copyText(t.getSelection()).then((ok) => flash(ok ? "Copied" : COPY_BLOCKED));
          t.clearSelection();
        }
        return false;
      }
      if (e.code === "KeyV") return false;
      if (e.shiftKey && e.code === "KeyA") {
        t.selectAll();
        return false;
      }
      return true;
    });
    // Right-click: a menu with copy, paste, select all and clear.
    const onContext = (ev: MouseEvent) => {
      ev.preventDefault();
      openMenu.current?.(ev.clientX, ev.clientY);
    };
    el.addEventListener("contextmenu", onContext);
    t.onData((d) => {
      if (mirrorOf) mirrorChan.current?.postMessage({ type: "in", data: d } satisfies RelayMsg);
      else sendInput(d);
    });
    t.onBinary((d) => {
      if (mirrorOf) {
        mirrorChan.current?.postMessage({ type: "in", data: d } satisfies RelayMsg);
        return;
      }
      const ws = socket.current;
      if (ws?.readyState === WebSocket.OPEN) ws.send(Uint8Array.from(d, (c) => c.charCodeAt(0)));
    });
    t.onResize(({ cols, rows }) => {
      if (mirrorOf) mirrorChan.current?.postMessage({ type: "resize", cols, rows } satisfies RelayMsg);
      else sendControl({ type: "resize", cols, rows });
      report({ cols, rows });
    });
    term.current = t;
    fit.current = f;
    search.current = s;
    serializer.current = ser;
    f.fit();

    let ping: ReturnType<typeof setInterval> | undefined;
    if (mirrorOf) {
      // Mirror: the owner window holds the SSH session.
      const bc = new BroadcastChannel(mirrorOf);
      mirrorChan.current = bc;
      bc.onmessage = (ev: MessageEvent<RelayMsg>) => {
        const m = ev.data;
        if (m.type === "snapshot") {
          t.reset();
          t.write(m.data);
          report(m.status);
          // Take over the size of this window.
          bc.postMessage({ type: "resize", cols: t.cols, rows: t.rows } satisfies RelayMsg);
          t.focus();
        } else if (m.type === "out") {
          t.write(m.data);
        } else if (m.type === "status") {
          report({ ...m.status, cols: t.cols, rows: t.rows });
        } else if (m.type === "gone") {
          t.writeln(`\r\n\x1b[33m— ${m.reason}\x1b[0m`);
          report({ state: "closed", message: m.reason });
        }
      };
      bc.postMessage({ type: "hello" } satisfies RelayMsg);
    } else {
      connect();
      ping = setInterval(() => sendControl({ type: "ping", t: performance.now() }), 5000);
    }

    const ro = new ResizeObserver(() => {
      if (el.offsetParent !== null) f.fit();
    });
    ro.observe(el);
    return () => {
      if (ping) clearInterval(ping);
      el.removeEventListener("contextmenu", onContext);
      ro.disconnect();
      mirrorChan.current?.close();
      mirrorChan.current = null;
      const ws = socket.current;
      socket.current = null;
      ws?.close();
      t.dispose();
    };
    // Settings changes are applied below without recreating the session.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [serverId, login, container, mirrorOf]);

  // Owner side of a pop-out.
  useEffect(() => {
    if (!relay || mirrorOf) return;
    const bc = new BroadcastChannel(relay);
    relayChan.current = bc;
    bc.onmessage = (ev: MessageEvent<RelayMsg>) => {
      const m = ev.data;
      const t = term.current;
      if (!t) return;
      if (m.type === "hello") {
        bc.postMessage({ type: "snapshot", data: serializer.current?.serialize() ?? "", status: status.current } satisfies RelayMsg);
      } else if (m.type === "in") {
        sendInput(m.data);
      } else if (m.type === "resize") {
        if (m.cols > 1 && m.rows > 1) t.resize(m.cols, m.rows);
      } else if (m.type === "reconnect") {
        t.reset();
        connect();
      } else if (m.type === "attach" || m.type === "terminate") {
        requestRef.current?.(m.type);
      }
    };
    const bye = () => bc.postMessage({ type: "gone", reason: "the main Forge window was closed; session ended" } satisfies RelayMsg);
    window.addEventListener("beforeunload", bye);
    return () => {
      window.removeEventListener("beforeunload", bye);
      relayChan.current = null;
      bc.close();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [relay, mirrorOf, connect]);

  useEffect(() => {
    const t = term.current;
    if (!t) return;
    t.options.theme = themeById(themeId).theme;
    t.options.fontSize = fontSize;
    if (host.current?.offsetParent !== null) fit.current?.fit();
  }, [themeId, fontSize]);

  useEffect(() => {
    const t = term.current;
    if (!t || !copyOnSelect) return;
    const d = t.onSelectionChange(() => {
      const sel = t.getSelection();
      if (sel) void copyText(sel);
    });
    return () => d.dispose();
  }, [copyOnSelect]);

  useEffect(() => {
    if (visible) {
      requestAnimationFrame(() => {
        fit.current?.fit();
        term.current?.focus();
      });
    }
  }, [visible]);

  useImperativeHandle(
    ref,
    () => ({
      focus: () => term.current?.focus(),
      reconnect: () => {
        term.current?.reset();
        connect();
      },
      findNext: (q) => search.current?.findNext(q, { decorations: { matchOverviewRuler: "#4cc3dd", activeMatchColorOverviewRuler: "#1f6fc4", activeMatchBackground: "#1f6fc4" } }) ?? false,
      findPrevious: (q) => search.current?.findPrevious(q) ?? false,
      clearSearch: () => search.current?.clearDecorations(),
      clear: () => term.current?.clear(),
      copySelection: async () => {
        const sel = term.current?.getSelection();
        if (sel) flash((await copyText(sel)) ? "Copied" : COPY_BLOCKED);
      },
      paste: async () => {
        const txt = await navigator.clipboard?.readText();
        if (txt) term.current?.paste(txt);
      },
      transcript: () => {
        const t = term.current;
        if (!t) return "";
        const buf = t.buffer.active;
        const lines: string[] = [];
        for (let i = 0; i < buf.length; i++) lines.push(buf.getLine(i)?.translateToString(true) ?? "");
        return lines.join("\n").replace(/\n+$/, "\n");
      },
    }),
    [connect],
  );

  return (
    <>
      <div ref={host} className="h-full w-full overflow-hidden px-2 py-1.5" style={{ background: themeById(themeId).theme.background }} />
      <PointerMenu menu={menu} onClose={() => setMenu(null)} />
    </>
  );
});
