import { Link } from "@tanstack/react-router";
import { ExternalLink, Plus, X } from "lucide-react";
import { useState } from "react";

import { useAllEnvironments } from "../../api/hooks";
import { type FileRef, type SecretEnvMode, type SwarmObjectKind, type SwarmSettings, defaultFileRef, useSwarmObjects } from "../../api/swarm";
import { IconButton, Input, Select } from "../ui";

const OCTAL_RE = /^[0-7]{3,4}$/;
const ID_RE = /^\d+$/;

let seq = 0;
const newId = () => `f${++seq}`;

/** Stable ids for rows of a list that may also change from outside. */
function useIds(n: number) {
  const [ids, setIds] = useState<string[]>(() => Array.from({ length: n }, newId));
  const current = ids.length === n ? ids : ids.length > n ? ids.slice(0, n) : [...ids, ...Array.from({ length: n - ids.length }, newId)];
  return { ids: current, add: () => setIds([...current, newId()]), remove: (i: number) => setIds(current.filter((_, j) => j !== i)) };
}

/** Swarm secrets and configs mounted as files, and how secret variables reach the task. */
export function SwarmFiles({ value, onChange, environmentId }: { value: SwarmSettings; onChange: (p: Partial<SwarmSettings>) => void; environmentId?: string }) {
  const env = (useAllEnvironments().data ?? []).find((e) => e.id === environmentId);
  const clusterId = env?.cluster_id ?? "";
  return (
    <div className="space-y-3 rounded-md border border-line p-3">
      <div className="flex flex-wrap items-baseline justify-between gap-2">
        <div className="text-xs font-medium">Secrets &amp; configs</div>
        {clusterId ? (
          <Link to="/clusters/$clusterId" params={{ clusterId }} search={{ tab: "swarm", section: "objects" }} className="inline-flex items-center gap-1 text-[11px] text-accent hover:underline">
            Create or rotate in the cluster's Swarm <ExternalLink className="size-3" />
          </Link>
        ) : (
          <span className="text-[11px] text-faint">Choose an environment to list its swarm's secrets and configs.</span>
        )}
      </div>
      <SecretEnv value={value.secret_env} onChange={(secret_env) => onChange({ secret_env })} />
      <Refs kind="secret" clusterId={clusterId} refs={value.secrets} onChange={(secrets) => onChange({ secrets })} />
      <Refs kind="config" clusterId={clusterId} refs={value.configs} onChange={(configs) => onChange({ configs })} />
    </div>
  );
}

function SecretEnv({ value, onChange }: { value: SecretEnvMode; onChange: (v: SecretEnvMode) => void }) {
  return (
    <fieldset className="space-y-1">
      <legend className="mb-1 text-[11px] font-medium text-muted">Secret variables</legend>
      <label className="flex items-start gap-2 text-xs">
        <input type="radio" name="secret-env" className="mt-0.5" checked={value === "files"} onChange={() => onChange("files")} />
        <span>
          As files (Swarm secrets) <span className="text-ok">recommended</span>
          <span className="block text-[11px] text-faint">
            Each secret variable <span className="font-mono">KEY</span> becomes a Swarm secret; the task gets <span className="font-mono">KEY_FILE=/run/secrets/KEY</span>. The app must read the file.
          </span>
        </span>
      </label>
      <label className="flex items-start gap-2 text-xs">
        <input type="radio" name="secret-env" className="mt-0.5" checked={value === "env"} onChange={() => onChange("env")} />
        <span>
          As environment variables
          <span className="block text-[11px] text-warn">Visible to anyone who can run docker service inspect on a manager.</span>
        </span>
      </label>
    </fieldset>
  );
}

function Refs({ kind, clusterId, refs, onChange }: { kind: SwarmObjectKind; clusterId: string; refs: FileRef[]; onChange: (r: FileRef[]) => void }) {
  const q = useSwarmObjects(clusterId, kind);
  const ids = useIds(refs.length);
  const names = [...new Set((q.data ?? []).map((o) => o.name))].sort();
  const noun = kind === "secret" ? "Secrets" : "Configs";
  const set = (i: number, p: Partial<FileRef>) => onChange(refs.map((r, j) => (j === i ? { ...r, ...p } : r)));
  const unused = names.find((n) => !refs.some((r) => r.name === n));
  return (
    <div className="space-y-1">
      <div className="flex items-center justify-between gap-2">
        <span className="text-[11px] font-medium text-muted">{noun}</span>
        <button
          type="button"
          className="inline-flex h-6 items-center gap-1 rounded-md border border-line px-2 text-[11px] text-muted hover:text-accent disabled:opacity-40"
          disabled={!names.length}
          title={names.length ? undefined : q.error ? "Could not list them" : `No ${kind}s in this swarm yet`}
          onClick={() => {
            ids.add();
            onChange([...refs, defaultFileRef(unused ?? names[0] ?? "")]);
          }}
        >
          <Plus className="size-3" /> Add
        </button>
      </div>
      {q.error && <div className="text-[11px] text-bad">Could not list {kind}s: {q.error instanceof Error ? q.error.message : String(q.error)}</div>}
      {refs.length === 0 ? (
        <div className="text-[11px] text-faint">None mounted.</div>
      ) : (
        refs.map((r, i) => {
          const modeText = r.mode.toString(8).padStart(4, "0");
          return (
            <div key={ids.ids[i]} className="flex flex-wrap items-center gap-1.5 sm:flex-nowrap">
              <Select value={r.name} onChange={(e) => set(i, { name: e.target.value })} aria-label={`${kind} name`} className="min-w-0 flex-1">
                {!names.includes(r.name) && <option value={r.name}>{r.name || "Choose…"}</option>}
                {names.map((n) => (
                  <option key={n} value={n}>
                    {n}
                  </option>
                ))}
              </Select>
              <Input
                className="h-8 min-w-0 flex-1 font-mono text-xs"
                aria-label="Target path"
                placeholder={kind === "secret" ? `/run/secrets/${r.name || "<name>"}` : `/${r.name || "<name>"}`}
                value={r.target ?? ""}
                onChange={(e) => set(i, { target: e.target.value })}
              />
              <Input className={`h-8 w-14 font-mono text-xs ${ID_RE.test(r.uid) ? "" : "border-bad"}`} aria-label="uid" title="uid" value={r.uid} onChange={(e) => set(i, { uid: e.target.value.trim() })} />
              <Input className={`h-8 w-14 font-mono text-xs ${ID_RE.test(r.gid) ? "" : "border-bad"}`} aria-label="gid" title="gid" value={r.gid} onChange={(e) => set(i, { gid: e.target.value.trim() })} />
              <ModeInput value={modeText} onChange={(m) => set(i, { mode: m })} />
              <IconButton
                type="button"
                label={`Remove ${r.name}`}
                className="size-7"
                onClick={() => {
                  ids.remove(i);
                  onChange(refs.filter((_, j) => j !== i));
                }}
              >
                <X className="size-3.5" />
              </IconButton>
            </div>
          );
        })
      )}
    </div>
  );
}

/** File mode as octal text; commits only a valid value. */
function ModeInput({ value, onChange }: { value: string; onChange: (m: number) => void }) {
  const [draft, setDraft] = useState<string | null>(null);
  const text = draft ?? value;
  const ok = OCTAL_RE.test(text);
  return (
    <Input
      className={`h-8 w-16 font-mono text-xs ${ok ? "" : "border-bad"}`}
      aria-label="Mode (octal)"
      title="File mode, octal (0444)"
      value={text}
      onChange={(e) => {
        const v = e.target.value.trim();
        setDraft(v);
        if (OCTAL_RE.test(v)) onChange(parseInt(v, 8));
      }}
      onBlur={() => setDraft(null)}
    />
  );
}
