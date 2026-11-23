/**
 * Asks which account to open a terminal as: the node's saved accounts, the
 * accounts the node reports ("on node"), the ones you used recently on it,
 * or another login. Searchable; Enter picks
 * the highlighted entry. "Save for this node" adds a typed login to the
 * node's list (needs server.manage; the server checks).
 */
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Search, SquareTerminal, UserRound } from "lucide-react";
import { useEffect, useMemo, useState } from "react";

import { api, get } from "../../api/client";
import { errorText, useToast } from "../../app/toast";
import { Button, Modal } from "../ui";

export interface PickNode {
  id: string;
  hostname: string;
  display_name?: string | null;
}

const RECENT_KEY = (id: string) => `forge-term-recent:${id}`;
const VALID = /^[A-Za-z0-9_.][A-Za-z0-9_.-]{0,31}$/;

export function recentLogins(nodeId: string): string[] {
  try {
    const v = JSON.parse(localStorage.getItem(RECENT_KEY(nodeId)) ?? "[]");
    return Array.isArray(v) ? v.filter((x): x is string => typeof x === "string").slice(0, 8) : [];
  } catch {
    return [];
  }
}

export function rememberLogin(nodeId: string, login: string) {
  try {
    const next = [login, ...recentLogins(nodeId).filter((l) => l !== login)].slice(0, 8);
    localStorage.setItem(RECENT_KEY(nodeId), JSON.stringify(next));
  } catch {
    /* storage unavailable: nothing to remember */
  }
}

export function LoginPicker({ node, onPick, onClose }: { node: PickNode | null; onPick: (login: string) => void; onClose: () => void }) {
  const qc = useQueryClient();
  const toast = useToast();
  const logins = useQuery({
    queryKey: ["server-logins", node?.id],
    queryFn: () => get<{ logins: string[]; saved?: string[]; discovered?: string[] }>(`/api/v1/servers/${node!.id}/logins`),
    enabled: !!node,
  }).data;
  const saved = logins ? (logins.saved ?? logins.logins) : undefined;
  const discovered = logins?.discovered;
  const [q, setQ] = useState("");
  const [active, setActive] = useState(0);
  const [save, setSave] = useState(false);

  useEffect(() => {
    setQ("");
    setActive(0);
    setSave(false);
  }, [node?.id]);

  const options = useMemo(() => {
    if (!node) return [];
    const recent = recentLogins(node.id);
    const all: { login: string; source: string }[] = [];
    for (const l of saved ?? []) all.push({ login: l, source: recent.includes(l) ? "saved · recent" : "saved" });
    for (const l of discovered ?? []) if (!all.some((o) => o.login === l)) all.push({ login: l, source: recent.includes(l) ? "on node · recent" : "on node" });
    for (const l of recent) if (!all.some((o) => o.login === l)) all.push({ login: l, source: "recent" });
    const needle = q.trim().toLowerCase();
    return needle ? all.filter((o) => o.login.toLowerCase().includes(needle)) : all;
  }, [node, saved, discovered, q]);

  const typed = q.trim();
  const typedIsNew = typed !== "" && !options.some((o) => o.login === typed);
  const typedValid = VALID.test(typed);

  const choose = async (login: string, persist: boolean) => {
    if (!node) return;
    if (persist && !(saved ?? []).includes(login)) {
      try {
        await api("PUT", `/api/v1/servers/${node.id}/logins`, { logins: [...(saved ?? []), login] });
        void qc.invalidateQueries({ queryKey: ["server-logins", node.id] });
      } catch (e) {
        toast(`Not saved for this node: ${errorText(e)}`, "error");
      }
    }
    rememberLogin(node.id, login);
    onPick(login);
  };

  const total = options.length + (typedIsNew ? 1 : 0);
  return (
    <Modal open={!!node} onOpenChange={(o) => !o && onClose()} title="Open terminal" description={node ? `Choose the account on ${node.display_name || node.hostname}.` : undefined}>
      <div className="space-y-3">
        <div className="flex items-center gap-1.5 rounded-md border border-line px-2.5 focus-within:border-accent">
          <Search className="size-3.5 text-faint" />
          <input
            autoFocus
            value={q}
            onChange={(e) => (setQ(e.target.value), setActive(0))}
            onKeyDown={(e) => {
              if (e.key === "ArrowDown") (e.preventDefault(), setActive((a) => Math.min(a + 1, Math.max(total - 1, 0))));
              else if (e.key === "ArrowUp") (e.preventDefault(), setActive((a) => Math.max(a - 1, 0)));
              else if (e.key === "Enter") {
                e.preventDefault();
                if (active < options.length) void choose(options[active]!.login, false);
                else if (typedIsNew && typedValid) void choose(typed, save);
              }
            }}
            placeholder="Search or type an account, e.g. deploy"
            aria-label="Account"
            data-bare className="h-9 w-full bg-transparent text-[13px] outline-none placeholder:text-faint"
          />
        </div>
        <div className="max-h-64 overflow-y-auto rounded-md border border-line py-1" role="listbox">
          {options.map((o, i) => (
            <button
              key={o.login}
              type="button"
              role="option"
              aria-selected={i === active}
              onMouseEnter={() => setActive(i)}
              onClick={() => void choose(o.login, false)}
              className={`flex w-full items-center gap-2 px-3 py-1.5 text-left text-[13px] ${i === active ? "bg-accent-soft" : ""}`}
            >
              <UserRound className="size-3.5 text-muted" />
              <span className="flex-1 font-mono">{o.login}</span>
              <span className="text-[11px] text-faint">{o.source}</span>
            </button>
          ))}
          {typedIsNew && (
            <button
              type="button"
              disabled={!typedValid}
              onMouseEnter={() => setActive(options.length)}
              onClick={() => void choose(typed, save)}
              className={`flex w-full items-center gap-2 px-3 py-1.5 text-left text-[13px] disabled:opacity-50 ${active === options.length ? "bg-accent-soft" : ""}`}
            >
              <SquareTerminal className="size-3.5 text-muted" />
              <span className="flex-1">
                Connect as <span className="font-mono">{typed}</span>
              </span>
              {!typedValid && <span className="text-[11px] text-bad">invalid name</span>}
            </button>
          )}
          {!total && <p className="px-3 py-2 text-[13px] text-muted">No saved accounts yet. Type the login you want to use.</p>}
        </div>
        {typedIsNew && typedValid && (
          <label className="flex items-center gap-2 text-xs text-muted">
            <input type="checkbox" checked={save} onChange={(e) => setSave(e.target.checked)} /> Save <span className="font-mono">{typed}</span> for this node (admins)
          </label>
        )}
        <div className="flex justify-end">
          <Button onClick={onClose}>Cancel</Button>
        </div>
      </div>
    </Modal>
  );
}
