// One health check: enable toggle, name, type, the probe's fields, timing
// and the collapsed advanced fields, with inline server errors and the
// server's plain-language summary underneath.
import { ChevronRight, Copy, Info, Plus, ShieldAlert, Trash2, X } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";

import type { HealthHeader, HealthProbe, HealthProbeType, HealthPurpose, HttpProbe, TcpProbe } from "../../api/types";
import { IconButton, Input, Select, Switch, TextArea } from "../ui";
import { type DraftCheck, MAX_ARGS, MAX_HEADERS, defaultProbe, isRemoteHost } from "./model";

/** Looks up the server's error for a path suffix of this check (`probe.port`). */
export type ErrLookup = (suffix: string) => string | undefined;

const TYPE_LABEL: Record<HealthProbeType, string> = { http: "HTTP endpoint", tcp: "TCP port", exec: "Command", shell: "Shell script" };

// ------------------------------------------------------------------ small controls

export function FieldError({ text }: { text?: string }) {
  if (!text) return null;
  return (
    <span role="alert" className="block text-[11px] leading-snug text-bad">
      {text}
    </span>
  );
}

function F({ label, error, hint, className = "", children }: { label: string; error?: string; hint?: ReactNode; className?: string; children: ReactNode }) {
  return (
    <label className={`block min-w-0 space-y-0.5 ${className}`}>
      <span className="text-[11px] font-medium text-muted">{label}</span>
      {children}
      {hint && !error && <span className="block text-[11px] text-faint">{hint}</span>}
      <FieldError text={error} />
    </label>
  );
}

const bad = (e?: string) => (e ? "border-bad" : "");
const dense = "py-1 text-xs";

/** A bounded whole-number input; a cleared box sends 0, so the server explains the limit. */
function NumInput({ value, onChange, min, max, unit, error, disabled, title, label }: { value: number; onChange: (n: number) => void; min: number; max: number; unit?: string; error?: string; disabled?: boolean; title?: string; label: string }) {
  const [text, setText] = useState(String(value));
  useEffect(() => {
    setText((t) => (Number(t || 0) === value ? t : String(value)));
  }, [value]);
  return (
    <span className="flex items-center gap-1" title={title}>
      <Input
        type="number"
        inputMode="numeric"
        aria-label={label}
        aria-invalid={!!error}
        min={min}
        max={max}
        step={1}
        disabled={disabled}
        className={`${dense} w-20 font-mono tabular-nums ${bad(error)}`}
        value={text}
        onChange={(e) => {
          const t = e.target.value;
          setText(t);
          const n = t.trim() === "" ? 0 : Math.trunc(Number(t));
          onChange(Number.isFinite(n) ? Math.min(Math.max(n, 0), max) : 0);
        }}
      />
      {unit && <span className="text-[11px] text-faint">{unit}</span>}
    </span>
  );
}

/** Comma-separated exit codes; keeps what you type while it isn't a list yet. */
function CodesInput({ value, onChange, error }: { value: number[]; onChange: (v: number[]) => void; error?: string }) {
  const fmt = (v: number[]) => v.join(", ");
  const [text, setText] = useState(fmt(value));
  useEffect(() => {
    setText((t) => (parseCodes(t).join(",") === value.join(",") ? t : fmt(value)));
  }, [value]);
  return (
    <Input
      aria-label="Expected exit codes"
      aria-invalid={!!error}
      className={`${dense} w-32 font-mono ${bad(error)}`}
      value={text}
      placeholder="0"
      onChange={(e) => {
        setText(e.target.value);
        onChange(parseCodes(e.target.value));
      }}
    />
  );
}
const parseCodes = (t: string) =>
  t
    .split(/[\s,]+/)
    .filter(Boolean)
    .map((x) => Math.trunc(Number(x)))
    .filter((n) => Number.isFinite(n))
    .map((n) => Math.max(-2147483648, Math.min(2147483647, n)));

function AddRow({ onClick, disabled, children, title }: { onClick: () => void; disabled?: boolean; children: ReactNode; title?: string }) {
  return (
    <button type="button" onClick={onClick} disabled={disabled} title={title} className="inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[11px] text-muted hover:bg-panel-2 hover:text-fg disabled:cursor-not-allowed disabled:opacity-50">
      <Plus className="size-3.5" /> {children}
    </button>
  );
}

function RemoveRow({ label, onClick }: { label: string; onClick: () => void }) {
  return (
    <IconButton type="button" label={label} className="size-7 shrink-0" onClick={onClick}>
      <X className="size-3.5" />
    </IconButton>
  );
}

// ------------------------------------------------------------------ probe parts

/** Host + port, and the "allow this host" opt-in when the host is remote. */
function Target({ p, set, err }: { p: HttpProbe | TcpProbe; set: (p: { host?: string; port?: number; allow_remote_host?: boolean }) => void; err: ErrLookup }) {
  const remote = isRemoteHost(p.host);
  return (
    <>
      <F label="Host" error={err("probe.host")} className="min-w-[10rem] flex-1">
        <Input className={`${dense} font-mono ${bad(err("probe.host"))}`} aria-invalid={!!err("probe.host")} placeholder="the workload" value={p.host ?? ""} onChange={(e) => set({ host: e.target.value.trim() })} spellCheck={false} />
      </F>
      <F label="Port" error={err("probe.port")}>
        <NumInput label="Port" value={p.port} min={1} max={65535} onChange={(port) => set({ port })} error={err("probe.port")} />
      </F>
      {(remote || p.allow_remote_host) && (
        <div className="basis-full space-y-0.5 rounded border border-line bg-panel-2 px-2 py-1.5">
          <label className="inline-flex items-center gap-2 text-xs">
            <input type="checkbox" checked={!!p.allow_remote_host} onChange={(e) => set({ allow_remote_host: e.target.checked })} />
            Allow probing this host
          </label>
          <p className="text-[11px] text-muted">
            The node connects to <span className="font-mono">{p.host || "—"}</span> instead of the workload. Metadata, link-local and IPv6 transition addresses (6to4, Teredo, NAT64) and other nodes of the fleet are always refused, when you save and again on the node.
          </p>
          <FieldError text={err("probe.allow_remote_host")} />
        </div>
      )}
    </>
  );
}

function HttpFields({ p, set, err }: { p: HttpProbe; set: (p: Partial<HttpProbe>) => void; err: ErrLookup }) {
  const statuses = p.expected_statuses;
  return (
    <div className="flex flex-wrap items-start gap-x-3 gap-y-2">
      <F label="Scheme" error={err("probe.scheme")}>
        <Select aria-label="Scheme" className={`${dense} w-24`} value={p.scheme} onChange={(e) => set({ scheme: e.target.value as HttpProbe["scheme"] })}>
          <option value="http">http</option>
          <option value="https">https</option>
        </Select>
      </F>
      <Target p={p} set={set} err={err} />
      <F label="Path" error={err("probe.path")} className="min-w-[10rem] flex-1">
        <Input className={`${dense} font-mono ${bad(err("probe.path"))}`} aria-invalid={!!err("probe.path")} value={p.path} placeholder="/" onChange={(e) => set({ path: e.target.value })} spellCheck={false} />
      </F>
      <F label="Method">
        <Select aria-label="Method" className={`${dense} w-24`} value={p.method} onChange={(e) => set({ method: e.target.value as HttpProbe["method"] })}>
          <option value="GET">GET</option>
          <option value="HEAD">HEAD</option>
        </Select>
      </F>
      <div className="min-w-0 space-y-0.5">
        <span className="text-[11px] font-medium text-muted">Expected status</span>
        <div className="flex flex-wrap items-center gap-1.5">
          {statuses.map((r, i) => (
            <span key={i} className="inline-flex items-center gap-0.5 rounded border border-line px-1 py-0.5">
              <NumInput label={`Status range ${i + 1} from`} value={r.from} min={100} max={599} onChange={(from) => set({ expected_statuses: statuses.map((x, j) => (j === i ? { ...x, from } : x)) })} />
              <span className="text-faint">–</span>
              <NumInput label={`Status range ${i + 1} to`} value={r.to} min={100} max={599} onChange={(to) => set({ expected_statuses: statuses.map((x, j) => (j === i ? { ...x, to } : x)) })} />
              {statuses.length > 1 && <RemoveRow label={`Remove status range ${i + 1}`} onClick={() => set({ expected_statuses: statuses.filter((_, j) => j !== i) })} />}
            </span>
          ))}
          <AddRow disabled={statuses.length >= 10} title={statuses.length >= 10 ? "At most 10 ranges" : undefined} onClick={() => set({ expected_statuses: [...statuses, { from: 200, to: 299 }] })}>
            Range
          </AddRow>
        </div>
        <FieldError text={err("probe.expected_statuses")} />
      </div>
    </div>
  );
}

function HttpAdvanced({ p, set, err }: { p: HttpProbe; set: (p: Partial<HttpProbe>) => void; err: ErrLookup }) {
  const headers = p.headers ?? [];
  const setH = (i: number, h: HealthHeader) => set({ headers: headers.map((x, j) => (j === i ? h : x)) });
  return (
    <div className="space-y-2">
      <div className="space-y-1">
        <span className="text-[11px] font-medium text-muted">Headers</span>
        {headers.map((h, i) => {
          const at = `probe.headers[${i}]`;
          const secret = h.secret !== undefined;
          return (
            <div key={i} className="space-y-0.5">
              <div className="flex flex-wrap items-center gap-1.5">
                <Input aria-label={`Header ${i + 1} name`} aria-invalid={!!err(`${at}.name`)} className={`${dense} w-40 font-mono ${bad(err(`${at}.name`))}`} placeholder="Accept" value={h.name} onChange={(e) => setH(i, { ...h, name: e.target.value })} spellCheck={false} />
                <Select aria-label={`Header ${i + 1} kind`} className={`${dense} w-32`} value={secret ? "secret" : "value"} onChange={(e) => setH(i, e.target.value === "secret" ? { name: h.name, secret: "vault:" } : { name: h.name, value: "" })}>
                  <option value="value">Value</option>
                  <option value="secret">Vault secret</option>
                </Select>
                {secret ? (
                  <Input aria-label={`Header ${i + 1} vault secret`} aria-invalid={!!err(`${at}.secret`)} className={`${dense} min-w-[10rem] flex-1 font-mono ${bad(err(`${at}.secret`))}`} placeholder="vault:<id>" value={h.secret ?? ""} onChange={(e) => setH(i, { name: h.name, secret: e.target.value.trim() })} spellCheck={false} />
                ) : (
                  <Input aria-label={`Header ${i + 1} value`} aria-invalid={!!err(`${at}.value`)} className={`${dense} min-w-[10rem] flex-1 font-mono ${bad(err(`${at}.value`))}`} placeholder="application/json" value={h.value ?? ""} onChange={(e) => setH(i, { name: h.name, value: e.target.value })} spellCheck={false} />
                )}
                <RemoveRow label={`Remove header ${i + 1}`} onClick={() => set({ headers: headers.filter((_, j) => j !== i) })} />
              </div>
              <FieldError text={err(`${at}.name`) ?? err(`${at}.value`) ?? err(`${at}.secret`) ?? err(at)} />
            </div>
          );
        })}
        <div className="flex flex-wrap items-center gap-2">
          <AddRow disabled={headers.length >= MAX_HEADERS} title={headers.length >= MAX_HEADERS ? `At most ${MAX_HEADERS} headers` : undefined} onClick={() => set({ headers: [...headers, { name: "", value: "" }] })}>
            Header
          </AddRow>
          <span className="text-[11px] text-faint">Credentials (Authorization, Cookie, names with token, key, secret, password or auth) must be a vault secret. The node asks Forge for the value just before each probe and keeps it in memory only; if you lose the right to use the item, the check turns Unknown. To a host other than the workload, a vault secret is sent only over HTTPS with certificate verification.</span>
        </div>
        <FieldError text={err("probe.headers")} />
      </div>
      {p.scheme === "https" ? (
        <div className="flex flex-wrap items-start gap-x-4 gap-y-2">
          <div className="pt-4">
            <Switch checked={p.tls_verify} onChange={(tls_verify) => set({ tls_verify })} label={<span className="text-xs">Verify TLS certificate</span>} />
            <FieldError text={err("probe.tls_verify")} />
          </div>
          <F label="Server name (SNI)" error={err("probe.server_name")} hint="When it differs from the host." className="min-w-[12rem] flex-1">
            <Input className={`${dense} font-mono ${bad(err("probe.server_name"))}`} aria-invalid={!!err("probe.server_name")} value={p.server_name ?? ""} onChange={(e) => set({ server_name: e.target.value.trim() })} spellCheck={false} />
          </F>
        </div>
      ) : (
        <p className="text-[11px] text-faint">TLS verification and server name apply to https.</p>
      )}
    </div>
  );
}

function ArgvEditor({ argv, set, error }: { argv: string[]; set: (a: string[]) => void; error?: string }) {
  const rows = argv.length ? argv : [""];
  return (
    <div className="space-y-1">
      <span className="text-[11px] font-medium text-muted">Program and arguments</span>
      {rows.map((a, i) => (
        <div key={i} className="flex items-center gap-1.5">
          <span className="w-16 shrink-0 text-[11px] text-faint">{i === 0 ? "Program" : `Arg ${i}`}</span>
          <Input
            aria-label={i === 0 ? "Program" : `Argument ${i}`}
            aria-invalid={i === 0 && !!error}
            className={`${dense} min-w-0 flex-1 font-mono ${i === 0 ? bad(error) : ""}`}
            placeholder={i === 0 ? "/usr/bin/pg_isready" : "--quiet"}
            value={a}
            onChange={(e) => set(rows.map((x, j) => (j === i ? e.target.value : x)))}
            spellCheck={false}
          />
          {i > 0 && <RemoveRow label={`Remove argument ${i}`} onClick={() => set(rows.filter((_, j) => j !== i))} />}
        </div>
      ))}
      <div className="flex flex-wrap items-center gap-2">
        <AddRow disabled={rows.length >= MAX_ARGS} title={rows.length >= MAX_ARGS ? `At most ${MAX_ARGS - 1} arguments` : undefined} onClick={() => set([...rows, ""])}>
          Argument
        </AddRow>
        <span className="text-[11px] text-faint">Run directly, never through a shell: no quoting or pipes.</span>
      </div>
      <FieldError text={error} />
    </div>
  );
}

// ------------------------------------------------------------------ the card

export function CheckCard({
  purpose,
  index,
  check,
  container,
  summary,
  err,
  hasErr,
  onChange,
  onDuplicate,
  onRemove,
  canDuplicate,
  embedded,
}: {
  purpose: HealthPurpose;
  index: number;
  check: DraftCheck;
  container: boolean;
  summary?: string;
  err: ErrLookup;
  /** Any error under these suffixes (opens the advanced fields). */
  hasErr: (suffixes: string[]) => boolean;
  onChange: (c: DraftCheck) => void;
  onDuplicate: () => void;
  onRemove: () => void;
  canDuplicate: boolean;
  /** Rendered under a list row that already has the enable toggle and the row menu: no box, no duplicate controls. */
  embedded?: boolean;
}) {
  const p = check.probe;
  const set = (patch: Partial<DraftCheck>) => onChange({ ...check, ...patch });
  const setProbe = (patch: Partial<HealthProbe>) => set({ probe: { ...p, ...patch } as HealthProbe });
  const switchType = (t: HealthProbeType) => {
    if (t === p.type) return;
    const stash = { ...check.stash, [p.type]: p };
    set({ probe: check.stash?.[t] ?? defaultProbe(t), stash });
  };
  const advancedKeys = ["initial_delay_secs", "probe.headers", "probe.server_name", "probe.tls_verify", "probe.expected_exit_codes", "probe.show_output"];
  const advErr = hasErr(advancedKeys);
  const [adv, setAdv] = useState(false);
  useEffect(() => {
    if (advErr) setAdv(true);
  }, [advErr]);
  const label = `${purpose} check ${check.name || index + 1}`;
  const runs = p.type === "exec" || p.type === "shell";
  const fixedSuccess = purpose !== "readiness";

  return (
    <div
      className={embedded ? `border-l-2 ${hasErr([""]) ? "border-bad/60" : "border-line-strong"} ${check.enabled ? "" : "opacity-75"}` : `rounded-md border ${hasErr([""]) ? "border-bad/50" : "border-line"} bg-panel ${check.enabled ? "" : "opacity-75"}`}
      role="group"
      aria-label={label}
    >
      <div className="flex flex-wrap items-start gap-x-3 gap-y-1.5 border-b border-line px-3 py-2">
        {!embedded && (
          <div className="pt-1">
            <Switch checked={check.enabled} onChange={(enabled) => set({ enabled })} label={<span className="sr-only">Enabled</span>} title={check.enabled ? "Enabled: turn off to keep it without running it" : "Disabled: kept but not run"} />
          </div>
        )}
        <div className="min-w-0 space-y-0.5">
          <Input aria-label="Check name" aria-invalid={!!err("name")} className={`${dense} w-44 font-mono ${bad(err("name"))}`} placeholder="name" value={check.name} onChange={(e) => set({ name: e.target.value.toLowerCase() })} spellCheck={false} maxLength={40} />
          <FieldError text={err("name")} />
        </div>
        <Select aria-label="Check type" className={`${dense} w-36`} value={p.type} onChange={(e) => switchType(e.target.value as HealthProbeType)}>
          {(Object.keys(TYPE_LABEL) as HealthProbeType[]).map((t) => (
            <option key={t} value={t}>
              {TYPE_LABEL[t]}
            </option>
          ))}
        </Select>
        {purpose === "readiness" && (
          <div className="pt-1">
            <Switch checked={check.required} onChange={(required) => set({ required })} label={<span className="text-xs">Required</span>} title={check.required ? "Failing makes the instance unready" : "Optional: failing shows degraded, the instance stays ready"} />
            <FieldError text={err("required")} />
          </div>
        )}
        {purpose !== "readiness" && err("required") && <FieldError text={err("required")} />}
        {!embedded && (
          <div className="ml-auto flex items-center">
            <IconButton type="button" label={canDuplicate ? "Duplicate check" : "Duplicate: section is full (10 checks)"} disabled={!canDuplicate} className="disabled:opacity-40" onClick={onDuplicate}>
              <Copy className="size-4" />
            </IconButton>
            <IconButton type="button" label="Remove check" className="hover:text-bad" onClick={onRemove}>
              <Trash2 className="size-4" />
            </IconButton>
          </div>
        )}
      </div>

      <div className="space-y-2.5 px-3 py-2.5">
        {p.type === "http" && <HttpFields p={p} set={setProbe} err={err} />}
        {p.type === "tcp" && (
          <div className="flex flex-wrap items-start gap-x-3 gap-y-2">
            <Target p={p} set={setProbe} err={err} />
          </div>
        )}
        {p.type === "exec" && <ArgvEditor argv={p.argv} set={(argv) => setProbe({ argv })} error={err("probe.argv")} />}
        {p.type === "shell" && (
          <div className="flex flex-wrap items-start gap-x-3 gap-y-2">
            <F label="Interpreter">
              <Select aria-label="Interpreter" className={`${dense} w-24`} value={p.interpreter} onChange={(e) => setProbe({ interpreter: e.target.value as "sh" | "bash" })}>
                <option value="sh">sh</option>
                <option value="bash">bash</option>
              </Select>
            </F>
            <F label="Script" error={err("probe.script")} className="min-w-[14rem] flex-1">
              <TextArea aria-invalid={!!err("probe.script")} className={`${bad(err("probe.script"))}`} rows={Math.min(10, Math.max(3, p.script.split("\n").length))} placeholder="test -r /run/myapp/heartbeat || exit 1" value={p.script} onChange={(e) => setProbe({ script: e.target.value })} spellCheck={false} />
            </F>
          </div>
        )}
        {runs && (
          <p className="flex items-start gap-1.5 rounded border border-warn/40 bg-warn-soft px-2 py-1 text-[11px] text-warn">
            <ShieldAlert className="mt-px size-3.5 shrink-0" />
            <span>
              Runs {container ? "inside the container, as its default user," : "as the app's own account"} every {check.period_secs || "—"} s on each node.{" "}
              {container
                ? "The script is visible in the process list of the container and of the node, so keep credentials out of it. "
                : "It can read and run files but can't create, write, truncate or delete any (Landlock); it can still change the mode of the app's own files and use the network. Everything it starts is ended with it. A node that can't confine it this way doesn't run it and reports Unknown. "}
              Saving a command or script needs an MFA-verified sign-in.
            </span>
          </p>
        )}

        <div className="flex flex-wrap items-start gap-x-4 gap-y-2">
          <F label="Every" error={err("period_secs")}>
            <NumInput label="Period" value={check.period_secs} min={1} max={3600} unit="s" error={err("period_secs")} onChange={(period_secs) => set({ period_secs })} />
          </F>
          <F label="Timeout" error={err("timeout_secs")}>
            <NumInput label="Timeout" value={check.timeout_secs} min={1} max={3600} unit="s" error={err("timeout_secs")} onChange={(timeout_secs) => set({ timeout_secs })} />
          </F>
          <F label="Fail after" error={err("failure_threshold")}>
            <NumInput label="Failure threshold" value={check.failure_threshold} min={1} max={100} unit="failures" error={err("failure_threshold")} onChange={(failure_threshold) => set({ failure_threshold })} />
          </F>
          <F label="Pass after" error={err("success_threshold")}>
            <NumInput
              label="Success threshold"
              value={check.success_threshold}
              min={1}
              max={fixedSuccess ? 1 : 100}
              unit="successes"
              error={err("success_threshold")}
              disabled={fixedSuccess && check.success_threshold === 1}
              title={fixedSuccess ? "Always 1 for startup and liveness checks" : undefined}
              onChange={(success_threshold) => set({ success_threshold })}
            />
          </F>
        </div>

        <div>
          <button type="button" aria-expanded={adv} onClick={() => setAdv((v) => !v)} className="inline-flex items-center gap-1 rounded text-[11px] font-medium text-muted hover:text-fg">
            <ChevronRight className={`size-3.5 transition-transform ${adv ? "rotate-90" : ""}`} />
            Advanced
            <span className="font-normal text-faint">{p.type === "http" ? "headers, TLS, initial delay" : p.type === "tcp" ? "initial delay" : "exit codes, initial delay"}</span>
            {advErr && !adv && <span className="size-1.5 rounded-full bg-bad" aria-label="has errors" />}
          </button>
          {adv && (
            <div className="mt-2 space-y-2 border-l-2 border-line pl-3">
              <div className="flex flex-wrap items-start gap-x-4 gap-y-2">
                <F label="Initial delay" error={err("initial_delay_secs")} hint="Wait before the first attempt.">
                  <NumInput label="Initial delay" value={check.initial_delay_secs} min={0} max={3600} unit="s" error={err("initial_delay_secs")} onChange={(initial_delay_secs) => set({ initial_delay_secs })} />
                </F>
                {(p.type === "exec" || p.type === "shell") && (
                  <F label="Expected exit codes" error={err("probe.expected_exit_codes")} hint="Comma-separated, 0–255.">
                    <CodesInput value={p.expected_exit_codes} onChange={(expected_exit_codes) => setProbe({ expected_exit_codes })} error={err("probe.expected_exit_codes")} />
                  </F>
                )}
              </div>
              {(p.type === "exec" || p.type === "shell") && (
                <div className="grid gap-0.5">
                  <Switch checked={!!p.show_output} onChange={(show_output) => setProbe({ show_output })} label={<span className="text-xs">Show the output of failed runs</span>} />
                  <span className="text-[11px] text-faint">
                    Off: a failed run reports its exit code only. On: the last 160 characters of its output are stored with the result and shown here. Leave it off if the command can print a secret.
                  </span>
                  <FieldError text={err("probe.show_output")} />
                </div>
              )}
              {p.type === "http" && <HttpAdvanced p={p} set={setProbe} err={err} />}
            </div>
          )}
        </div>
      </div>

      {!embedded && (
        <div className="flex items-start gap-1.5 border-t border-line px-3 py-1.5 text-[12px] text-muted">
          <Info className="mt-0.5 size-3.5 shrink-0 text-faint" />
          <span className="min-w-0 break-words">{summary ? `${check.enabled ? "" : "Disabled. "}${summary}` : "—"}</span>
        </div>
      )}
    </div>
  );
}
