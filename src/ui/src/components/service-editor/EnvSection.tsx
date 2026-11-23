import { ClipboardPaste, Eye, EyeOff, FileCode2, KeyRound, LayoutGrid, Lock, Plus, Trash2 } from "lucide-react";
import { useState } from "react";

import type { CatalogPackage, ServiceInput } from "../../api/types";
import { Badge, Button, IconButton, Input, Modal, Mono, TextArea } from "../ui";
import { KEY_RE, SEALED, looksSecret, parseDotenv } from "./sections";

type EnvRow = ServiceInput["env"][number];

/** Grid or bulk-text view toggle; lives in the card header. */
export function EnvModeToggle({ mode, onChange }: { mode: "grid" | "text"; onChange: (m: "grid" | "text") => void }) {
  return (
    <div className="flex rounded-md border border-line p-0.5 text-xs" role="tablist" aria-label="Variables view">
      {(
        [
          ["grid", LayoutGrid, "Grid"],
          ["text", FileCode2, "Bulk edit"],
        ] as const
      ).map(([id, Icon, label]) => (
        <button key={id} type="button" role="tab" aria-selected={mode === id} onClick={() => onChange(id)} className={`inline-flex items-center gap-1 rounded px-2 py-0.5 ${mode === id ? "bg-accent-soft text-accent" : "text-muted hover:text-fg"}`}>
          <Icon className="size-3.5" /> {label}
        </button>
      ))}
    </div>
  );
}

const toText = (rows: EnvRow[]) => rows.map((e) => `${e.key}=${e.secret && e.value === null ? SEALED : (e.value ?? "")}`).join("\n");

/** Rows from text, keeping stored secrets written as `<sealed>`. */
function fromText(t: string, old: EnvRow[]): EnvRow[] {
  return parseDotenv(t).map(({ key, value }) => {
    const prev = old.find((e) => e.key === key);
    return value === SEALED && prev?.secret ? { key, value: null, secret: true } : { key, value, secret: prev?.secret ?? looksSecret(key) };
  });
}

/**
 * Environment variables as a dense editable table (or `.env` text), plus the
 * installed packages whose variables the service also loads.
 */
export function EnvSection({ value, set, mode, onMode, packages }: { value: ServiceInput; set: (p: Partial<ServiceInput>) => void; mode: "grid" | "text"; onMode: (m: "grid" | "text") => void; packages: CatalogPackage[] }) {
  const rows = value.env;
  const [text, setText] = useState(() => toText(rows));
  const [shown, setShown] = useState<Set<number>>(new Set());
  const [paste, setPaste] = useState<string | null>(null);
  const setRows = (env: EnvRow[]) => set({ env });
  const setRow = (i: number, p: Partial<EnvRow>) => setRows(rows.map((e, j) => (j === i ? { ...e, ...p } : e)));
  const dupes = new Set(rows.map((e) => e.key).filter((k, i, a) => k && a.indexOf(k) !== i));
  const merge = (t: string) => {
    const env = [...rows];
    for (const { key, value: v } of parseDotenv(t)) {
      const at = env.findIndex((e) => e.key === key);
      const row = { key, value: v, secret: looksSecret(key) || (at >= 0 && !!env[at]?.secret) };
      if (at >= 0) env[at] = row;
      else env.push(row);
    }
    setRows(env);
  };
  const switchTo = (m: "grid" | "text") => {
    if (m === mode) return;
    if (m === "text") setText(toText(rows));
    else setRows(fromText(text, rows));
    onMode(m);
  };
  const toggleShown = (i: number) =>
    setShown((s) => {
      const n = new Set(s);
      if (n.has(i)) n.delete(i);
      else n.add(i);
      return n;
    });
  const pkgs = packages.filter((p) => p.category === "databases" || p.category === "storage");

  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center gap-2">
        <EnvModeToggle mode={mode} onChange={switchTo} />
        {mode === "grid" && (
          <>
            <Button type="button" variant="ghost" onClick={() => setPaste("")}>
              <ClipboardPaste className="size-4" /> Paste .env
            </Button>
            <Button type="button" onClick={() => setRows([...rows, { key: "", value: "", secret: false }])}>
              <Plus className="size-4" /> Variable
            </Button>
          </>
        )}
        <span className="ml-auto text-xs text-faint">
          {rows.length ? `${rows.length} variable${rows.length === 1 ? "" : "s"}${rows.some((e) => e.secret) ? ` · ${rows.filter((e) => e.secret).length} secret` : ""}` : "—"}
        </span>
      </div>
      {mode === "text" ? (
        <>
          <TextArea
            aria-label="Variables as .env text"
            rows={Math.max(8, text.split("\n").length + 1)}
            value={text}
            onChange={(e) => setText(e.target.value)}
            onBlur={() => setRows(fromText(text, rows))}
            placeholder={"DATABASE_URL=postgres://…\nDEBUG=false"}
            spellCheck={false}
          />
          <p className="text-xs text-faint">
            One <Mono>KEY=value</Mono> per line. <Mono>{SEALED}</Mono> keeps a stored secret. New names that look secret are sealed.
          </p>
        </>
      ) : rows.length === 0 ? (
        <p className="rounded-md border border-dashed border-line px-3 py-2 text-xs text-faint">No variables yet. Add one, or paste a .env file.</p>
      ) : (
        <div className="overflow-x-auto rounded-md border border-line">
          <table className="w-full min-w-[520px] text-[13px]">
            <thead className="bg-panel-2 text-left text-[11px] tracking-wide text-faint uppercase">
              <tr>
                <th className="w-[36%] px-2 py-1 font-medium">Name</th>
                <th className="px-2 py-1 font-medium">Value</th>
                <th className="w-[104px] px-2 py-1 text-right font-medium">Secret</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-line">
              {rows.map((e, i) => {
                const bad = (!!e.key && !KEY_RE.test(e.key)) || dupes.has(e.key);
                const stored = e.secret && e.value === null;
                return (
                  // Index keys: a row's name is what the user is editing, so it cannot be the key.
                  <tr key={i}>
                    <td className="px-2 py-1">
                      <Input
                        aria-label={`Name of variable ${i + 1}`}
                        aria-invalid={bad || undefined}
                        className={`h-7 py-0 font-mono text-xs ${bad ? "border-bad" : ""}`}
                        value={e.key}
                        placeholder="NAME"
                        spellCheck={false}
                        onChange={(ev) => setRow(i, { key: ev.target.value.toUpperCase().replace(/[^A-Z0-9_]/g, "_"), secret: e.secret || looksSecret(ev.target.value) })}
                      />
                    </td>
                    <td className="px-2 py-1">
                      <Input
                        aria-label={`Value of ${e.key || `variable ${i + 1}`}`}
                        className="h-7 py-0 font-mono text-xs"
                        type={e.secret && !shown.has(i) ? "password" : "text"}
                        autoComplete="off"
                        value={e.value ?? ""}
                        placeholder={stored ? "•••••• stored; type to replace" : "value"}
                        spellCheck={false}
                        onChange={(ev) => setRow(i, { value: ev.target.value })}
                      />
                    </td>
                    <td className="px-1 py-1">
                      <div className="flex justify-end">
                        {e.secret && (
                          <IconButton type="button" className="size-7" label={shown.has(i) ? "Hide value" : "Show value"} onClick={() => toggleShown(i)}>
                            {shown.has(i) ? <EyeOff className="size-3.5" /> : <Eye className="size-3.5" />}
                          </IconButton>
                        )}
                        <IconButton type="button" label={e.secret ? "Secret (sealed); click to make plain" : "Mark as secret"} className={`size-7 ${e.secret ? "text-warn" : ""}`} onClick={() => setRow(i, { secret: !e.secret })}>
                          {e.secret ? <Lock className="size-3.5" /> : <KeyRound className="size-3.5" />}
                        </IconButton>
                        <IconButton type="button" label={`Remove ${e.key || `variable ${i + 1}`}`} className="size-7 hover:text-bad" onClick={() => setRows(rows.filter((_, j) => j !== i))}>
                          <Trash2 className="size-3.5" />
                        </IconButton>
                      </div>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
      <div className="border-t border-line pt-3">
        <p className="mb-1.5 text-xs font-medium text-muted">Package variables</p>
        <p className="mb-2 text-xs text-faint">
          Also load an installed package's <Mono>/etc/forge/pkgs/&lt;package&gt;.env</Mono> (for example <Mono>FORGE_PKG_POSTGRESQL_URL</Mono>), before your own variables.
        </p>
        {pkgs.length === 0 ? (
          <span className="text-xs text-faint">—</span>
        ) : (
          <div className="flex flex-wrap gap-1.5">
            {pkgs.map((p) => {
              const on = value.env_files.includes(p.id);
              return (
                <button
                  key={p.id}
                  type="button"
                  aria-pressed={on}
                  onClick={() => set({ env_files: on ? value.env_files.filter((x) => x !== p.id) : [...value.env_files, p.id] })}
                  className={`rounded-full border px-2.5 py-0.5 text-xs ${on ? "border-accent bg-accent-soft text-accent" : "border-line text-muted hover:text-fg"}`}
                >
                  {p.name}
                </button>
              );
            })}
          </div>
        )}
      </div>
      {paste !== null && (
        <Modal open onOpenChange={(o) => !o && setPaste(null)} title="Paste .env" description="KEY=value lines; comments and export prefixes are fine. Existing names are updated.">
          <TextArea aria-label=".env text" rows={10} value={paste} onChange={(e) => setPaste(e.target.value)} placeholder={"DATABASE_URL=postgres://…\nDEBUG=false"} />
          <div className="mt-3 flex items-center justify-between">
            <Badge>{parseDotenv(paste).length} variables</Badge>
            <Button
              type="button"
              variant="primary"
              onClick={() => {
                merge(paste);
                setPaste(null);
              }}
            >
              Import
            </Button>
          </div>
        </Modal>
      )}
    </div>
  );
}
