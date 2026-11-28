import { useSearch } from "@tanstack/react-router";
import { PlugZap, SquareTerminal, Undo2, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { loadTermPrefs } from "../app/terminals";
import { stateDot } from "../components/terminal/TerminalDock";
import { TerminalPane, relayName, type PaneStatus, type RelayMsg } from "../components/terminal/TerminalPane";
import { Badge, Button } from "../components/ui";

/**
 * A terminal session popped out of the dock. It mirrors the live session owned
 * by the main window; if that window goes away it can reconnect on its own.
 */
export function TerminalPopout() {
  const q = useSearch({ from: "/popout/terminal" });
  const [prefs] = useState(loadTermPrefs);
  const [status, setStatus] = useState<PaneStatus>({ state: "connecting" });
  const [standalone, setStandalone] = useState(false);
  const leaving = useRef(false);
  const chan = relayName(q.key);

  const label = q.container ? `${q.cname || q.container} @ ${q.host}` : `${q.login}@${q.host}`;
  useEffect(() => {
    document.title = `${label} · Forge terminal`;
  }, [label]);

  const send = (m: RelayMsg) => {
    const bc = new BroadcastChannel(chan);
    bc.postMessage(m);
    bc.close();
  };

  // Closing this window returns the session to the dock (it keeps running).
  useEffect(() => {
    const bye = () => {
      if (!leaving.current && !standalone) send({ type: "attach" });
    };
    window.addEventListener("beforeunload", bye);
    return () => window.removeEventListener("beforeunload", bye);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [standalone]);

  const ownerGone = !standalone && status.state === "closed" && !!status.message?.includes("main Forge window");

  return (
    <div className="flex h-screen flex-col bg-panel">
      <div className="flex h-10 shrink-0 items-center gap-2 border-b border-line bg-panel-2/60 px-3 text-[13px]">
        <SquareTerminal className="size-4 text-accent" />
        <span className={`size-2 rounded-full ${stateDot[status.state]}`} />
        <span className="font-mono">{label}</span>
        {q.container ? <Badge tone="accent">container</Badge> : q.role === "control" && <Badge tone="accent">control</Badge>}
        <span className="text-xs text-faint">{standalone ? "own connection" : "mirrored from the main window"}</span>
        <span className="ml-auto flex gap-1">
          {ownerGone && (
            <Button onClick={() => setStandalone(true)}>
              <PlugZap className="size-4" /> Reconnect here
            </Button>
          )}
          {!standalone && (
            <Button
              title="Return this session to the main window's terminal panel"
              onClick={() => {
                leaving.current = true;
                send({ type: "attach" });
                window.close();
              }}
            >
              <Undo2 className="size-4" /> Back to panel
            </Button>
          )}
          <Button
            variant="ghost"
            title={q.container ? "End the container shell" : "End the SSH session"}
            onClick={() => {
              leaving.current = true;
              if (!standalone) send({ type: "terminate" });
              window.close();
            }}
          >
            <X className="size-4" /> End session
          </Button>
        </span>
      </div>
      <div className="flex min-h-0 flex-1">
        <TerminalPane
          key={standalone ? "own" : "mirror"}
          serverId={q.node}
          login={q.login}
          container={q.container || undefined}
          themeId={prefs.themeId}
          fontSize={prefs.fontSize}
          copyOnSelect={prefs.copyOnSelect}
          visible
          mirrorOf={standalone ? undefined : chan}
          onStatus={setStatus}
        />
      </div>
      <div className="flex h-6 shrink-0 items-center gap-4 border-t border-line bg-panel-2/60 px-3 font-mono text-[11px] text-faint">
        <span>{status.state}</span>
        {status.cols && (
          <span>
            {status.cols}×{status.rows}
          </span>
        )}
        {status.latencyMs !== undefined && <span>{status.latencyMs} ms</span>}
        <span className="ml-auto">Closing this window moves the session back to the panel</span>
      </div>
    </div>
  );
}
