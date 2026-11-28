import { Link, useNavigate, useSearch } from "@tanstack/react-router";
import { nodeName } from "../api/types";
import { ExternalLink, SquareTerminal, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { useClusters, useMe, useServers } from "../api/hooks";
import { isShell, useTerminals } from "../app/terminals";
import { Pager, usePaged } from "../components/Pager";
import { Badge, Button, Empty, Input, PageHeader, Panel, SearchInput, Status, Table, Td } from "../components/ui";

/**
 * Terminal launcher. Sessions themselves live in the bottom panel (they keep
 * running while you navigate) or in pop-out windows.
 */
export function TerminalPage() {
  const search = useSearch({ from: "/terminal" });
  const navigate = useNavigate();
  const nodes = useServers().data ?? [];
  const clusters = useClusters().data ?? [];
  const me = useMe().data;
  const t = useTerminals();
  const shells = t.sessions.filter(isShell);
  const [q, setQ] = useState("");
  const [login, setLogin] = useState(t.prefs.lastLogin);

  // Deep link: /terminal?node=<id>&login=<user> opens a session once.
  const opened = useRef(false);
  useEffect(() => {
    const n = nodes.find((x) => x.id === search.node);
    if (opened.current || !n) return;
    opened.current = true;
    t.open(n, search.login);
    void navigate({ to: "/terminal", search: {}, replace: true });
  }, [search.node, search.login, nodes, t, navigate]);

  const usable = nodes.filter((n) => n.status !== "revoked");
  const needle = q.trim().toLowerCase();
  const rows = usable.filter((n) => `${nodeName(n)} ${n.hostname}`.toLowerCase().includes(needle));
  const paged = usePaged(rows, { reset: needle });

  return (
    <>
      <PageHeader
        eyebrow="Access"
        title="Terminal"
        description="SSH into any node. Sessions open in the panel at the bottom and stay connected while you move around the console; pop one out into its own window when you need more room."
      />
      <div className="grid gap-4 xl:grid-cols-12">
        <div className="xl:col-span-8">
          <Panel
            title="Connect to a node"
            actions={
              <div className="flex flex-wrap items-center gap-2">
                <label className="flex items-center gap-2 text-xs text-muted">
                  Login
                  <Input className="w-32 py-1" value={login} onChange={(e) => setLogin(e.target.value)} pattern="[A-Za-z0-9_][A-Za-z0-9_.\-]{0,31}" />
                </label>
                <SearchInput value={q} onChange={setQ} placeholder="Filter nodes" />
              </div>
            }
          >
            {rows.length === 0 ? (
              <Empty icon={<SquareTerminal className="size-5" />} title={usable.length ? "No node matches" : "No nodes"}>
                {usable.length ? "Change the filter above." : "Add a node first."}
              </Empty>
            ) : (
              <>
              <Table head={["Node", "Status", "Cluster", ""]}>
                {paged.rows.map((n) => (
                  <tr key={n.id} className="hover:bg-panel-2/60">
                    <Td>
                      <Link to="/nodes/$nodeId" params={{ nodeId: n.id }} className="mr-2 font-medium hover:text-accent">
                        {nodeName(n)}
                      </Link>
                      <Badge tone={n.role === "control" ? "accent" : "muted"}>{n.role === "control" ? "Control" : "Worker"}</Badge>
                    </Td>
                    <Td>
                      <Status value={n.status} />
                    </Td>
                    <Td className="text-muted">{clusters.find((c) => c.id === n.cluster_id)?.name ?? "None"}</Td>
                    <Td className="text-right">
                      <Button variant="primary" onClick={() => t.open(n, login)}>
                        <SquareTerminal className="size-4" /> Connect
                      </Button>
                    </Td>
                  </tr>
                ))}
              </Table>
              <Pager paged={paged} />
              </>
            )}
          </Panel>
        </div>
        <div className="xl:col-span-4">
          <Panel title={`Open sessions (${shells.length})`} description={me && !me.mfa ? "Sessions require an MFA-verified sign-in." : "Certificates are valid for five minutes; every session is audited."}>
            <ul className="divide-y divide-line">
              {shells.map((s) => (
                <li key={s.key} className="flex items-center gap-2 px-4 py-2 text-[13px]">
                  <button
                    className="min-w-0 flex-1 truncate text-left font-mono text-xs hover:text-accent"
                    onClick={() => {
                      if (s.detached) window.open("", `forge-term-${s.key}`)?.focus();
                      else {
                        t.activate(s.key);
                        t.setDock({ open: true });
                      }
                    }}
                  >
                    {s.login}@{s.hostname}
                  </button>
                  {s.detached ? <Badge tone="accent">window</Badge> : <Badge>panel</Badge>}
                  {!s.detached && (
                    <button aria-label="Open in a separate window" className="text-faint hover:text-fg" onClick={() => t.detach(s.key)}>
                      <ExternalLink className="size-4" />
                    </button>
                  )}
                  <button aria-label="Close session" className="text-faint hover:text-bad" onClick={() => t.close(s.key)}>
                    <X className="size-4" />
                  </button>
                </li>
              ))}
              {shells.length === 0 && <li className="px-4 py-6 text-muted">No open sessions.</li>}
            </ul>
          </Panel>
        </div>
      </div>
    </>
  );
}
