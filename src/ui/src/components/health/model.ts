// Draft model of the Health Checks tab: wire <-> draft conversion, presets,
// the YAML view's parser (with an unknown-key check that mirrors the
// server's `deny_unknown_fields`), and the review diff.
//
// The wire shape is `HealthPolicy` from `api/types.ts`. Drafts add UI-only
// keys (`key`, `stash`) that `toPolicy` strips: the server refuses extra keys.
import yaml from "js-yaml";

import type { HealthCheck, HealthCheckSpec, HealthHeader, HealthPolicy, HealthProbe, HealthProbeType, HealthPurpose } from "../../api/types";

export const PURPOSES: HealthPurpose[] = ["startup", "readiness", "liveness"];
export const MAX_CHECKS = 10;
export const MAX_HEADERS = 20;
export const MAX_ARGS = 64;

export interface DraftCheck extends HealthCheckSpec {
  /** UI-only: stable React key. */
  key: string;
  /** UI-only: the probe last used per type, restored when switching back. */
  stash?: Partial<Record<HealthProbeType, HealthProbe>>;
}
export type Draft = Record<HealthPurpose, DraftCheck[]> & {
  /** Forge's default checks; `default_checks` on the wire (absent = on). */
  defaultChecks: boolean;
};

let seq = 0;
export const newKey = () => `c${++seq}`;

// ------------------------------------------------------------------ defaults

export function defaultProbe(t: HealthProbeType): HealthProbe {
  switch (t) {
    case "http":
      return { type: "http", scheme: "http", host: "", allow_remote_host: false, port: 8080, path: "/health", method: "GET", headers: [], expected_statuses: [{ from: 200, to: 299 }], tls_verify: true, server_name: "" };
    case "tcp":
      return { type: "tcp", host: "", allow_remote_host: false, port: 8080 };
    case "exec":
      return { type: "exec", argv: [""], expected_exit_codes: [0] };
    case "shell":
      return { type: "shell", interpreter: "sh", script: "", expected_exit_codes: [0] };
  }
}

export const PRESETS: { type: HealthProbeType; label: string; name: string }[] = [
  { type: "http", label: "HTTP endpoint", name: "http" },
  { type: "tcp", label: "TCP port", name: "tcp" },
  { type: "exec", label: "Command", name: "command" },
  { type: "shell", label: "Shell script", name: "script" },
];

/** A unique `base`, `base-2`, … within a section. */
export function uniqueName(list: { name: string }[], base: string): string {
  const taken = new Set(list.map((c) => c.name));
  if (!taken.has(base)) return base;
  for (let i = 2; ; i++) if (!taken.has(`${base}-${i}`)) return `${base}-${i}`;
}

/** A new check from a preset: workload-local target, defaults only, nothing remote or privileged. */
export function presetCheck(list: DraftCheck[], t: HealthProbeType): DraftCheck {
  const preset = PRESETS.find((p) => p.type === t) ?? { name: "check" };
  return { key: newKey(), name: uniqueName(list, preset.name), enabled: true, required: true, probe: defaultProbe(t), period_secs: 10, timeout_secs: 2, initial_delay_secs: 0, failure_threshold: 3, success_threshold: 1 };
}

/**
 * The Docker HEALTHCHECK as a liveness shell check. A Docker value of 0
 * means "Docker's default" (30 s interval, 30 s timeout, 3 retries), so 0 is
 * mapped to that default rather than to an invalid 0.
 */
export function legacyCheck(list: DraftCheck[], h: HealthCheck): DraftCheck {
  const or = (v: unknown, d: number) => (typeof v === "number" && v > 0 ? Math.floor(v) : d);
  return {
    key: newKey(),
    name: uniqueName(list, "docker-healthcheck"),
    enabled: true,
    required: true,
    probe: { type: "shell", interpreter: "sh", script: typeof h.command === "string" ? h.command : "", expected_exit_codes: [0] },
    period_secs: or(h.interval_secs, 30),
    // Docker allows timeout = interval (its defaults are 30 s and 30 s); Forge
    // needs the timeout shorter than the period.
    timeout_secs: Math.max(1, Math.min(or(h.timeout_secs, 30), or(h.interval_secs, 30) - 1)),
    initial_delay_secs: typeof h.start_period_secs === "number" && h.start_period_secs > 0 ? Math.floor(h.start_period_secs) : 0,
    failure_threshold: or(h.retries, 3),
    success_threshold: 1,
  };
}

// ------------------------------------------------------------------ wire <-> draft

const num = (v: unknown, d: number) => (typeof v === "number" && Number.isFinite(v) ? v : d);
const str = (v: unknown, d = "") => (typeof v === "string" ? v : d);
const bool = (v: unknown, d: boolean) => (typeof v === "boolean" ? v : d);
const arr = <T>(v: unknown): T[] => (Array.isArray(v) ? (v as T[]) : []);

/** Fills every default the server would (serde defaults), from a loose object. */
function normProbe(p: Partial<HealthProbe> | undefined): HealthProbe {
  const o = (p ?? {}) as Record<string, unknown>;
  switch (o.type) {
    case "tcp":
      return { type: "tcp", host: str(o.host), allow_remote_host: bool(o.allow_remote_host, false), port: num(o.port, 0) };
    case "exec":
      return { type: "exec", argv: arr<unknown>(o.argv).map((a) => str(a)), expected_exit_codes: o.expected_exit_codes === undefined ? [0] : arr<number>(o.expected_exit_codes), show_output: bool(o.show_output, false) };
    case "shell":
      return { type: "shell", interpreter: o.interpreter === "bash" ? "bash" : "sh", script: str(o.script), expected_exit_codes: o.expected_exit_codes === undefined ? [0] : arr<number>(o.expected_exit_codes), show_output: bool(o.show_output, false) };
    default:
      return {
        type: "http",
        scheme: o.scheme === "https" ? "https" : "http",
        host: str(o.host),
        allow_remote_host: bool(o.allow_remote_host, false),
        port: num(o.port, 0),
        path: str(o.path, "/"),
        method: o.method === "HEAD" ? "HEAD" : "GET",
        headers: arr<HealthHeader>(o.headers).map((h) => ({ name: str(h?.name), ...(h?.value !== undefined ? { value: str(h.value) } : {}), ...(h?.secret !== undefined ? { secret: str(h.secret) } : {}) })),
        expected_statuses: o.expected_statuses === undefined ? [{ from: 200, to: 299 }] : arr<{ from: number; to: number }>(o.expected_statuses).map((r) => ({ from: num(r?.from, 0), to: num(r?.to, 0) })),
        tls_verify: bool(o.tls_verify, true),
        server_name: str(o.server_name),
      };
  }
}

function normCheck(c: Partial<HealthCheckSpec>): HealthCheckSpec {
  return {
    name: str(c?.name),
    enabled: bool(c?.enabled, true),
    required: bool(c?.required, true),
    probe: normProbe(c?.probe),
    period_secs: num(c?.period_secs, 10),
    timeout_secs: num(c?.timeout_secs, 2),
    initial_delay_secs: num(c?.initial_delay_secs, 0),
    failure_threshold: num(c?.failure_threshold, 3),
    success_threshold: num(c?.success_threshold, 1),
  };
}

export function toDraft(p: HealthPolicy | undefined): Draft {
  const out = { defaultChecks: p?.default_checks !== false } as Draft;
  for (const k of PURPOSES) out[k] = arr<Partial<HealthCheckSpec>>(p?.[k]).map((c) => ({ ...normCheck(c), key: newKey() }));
  return out;
}

/** Exactly the server's keys, in a stable order; UI-only keys are dropped. */
function wireProbe(p: HealthProbe): HealthProbe {
  switch (p.type) {
    case "http":
      return {
        type: "http",
        scheme: p.scheme,
        host: p.host ?? "",
        allow_remote_host: !!p.allow_remote_host,
        port: p.port,
        path: p.path,
        method: p.method,
        headers: (p.headers ?? []).map((h) => ({ name: h.name, ...(h.value !== undefined ? { value: h.value } : {}), ...(h.secret !== undefined ? { secret: h.secret } : {}) })),
        expected_statuses: p.expected_statuses.map((r) => ({ from: r.from, to: r.to })),
        tls_verify: p.tls_verify,
        server_name: p.server_name ?? "",
      };
    case "tcp":
      return { type: "tcp", host: p.host ?? "", allow_remote_host: !!p.allow_remote_host, port: p.port };
    case "exec":
      return { type: "exec", argv: [...p.argv], expected_exit_codes: [...p.expected_exit_codes], show_output: !!p.show_output };
    case "shell":
      return { type: "shell", interpreter: p.interpreter, script: p.script, expected_exit_codes: [...p.expected_exit_codes], show_output: !!p.show_output };
  }
}

export function wireCheck(c: HealthCheckSpec): HealthCheckSpec {
  return {
    name: c.name,
    enabled: c.enabled,
    required: c.required,
    probe: wireProbe(c.probe),
    period_secs: c.period_secs,
    timeout_secs: c.timeout_secs,
    initial_delay_secs: c.initial_delay_secs,
    failure_threshold: c.failure_threshold,
    success_threshold: c.success_threshold,
  };
}

/** The canonical wire policy (every key explicit, so two equal policies stringify equally). */
export function toPolicy(d: Draft): HealthPolicy {
  return { ...(d.defaultChecks ? {} : { default_checks: false }), startup: d.startup.map(wireCheck), readiness: d.readiness.map(wireCheck), liveness: d.liveness.map(wireCheck) };
}

/** The policy without default values: what the YAML view shows. */
export function compact(p: HealthPolicy): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  if (p.default_checks === false) out.default_checks = false;
  for (const k of PURPOSES) {
    const list = p[k] ?? [];
    if (!list.length) continue;
    out[k] = list.map((c) => {
      const o: Record<string, unknown> = { name: c.name };
      if (!c.enabled) o.enabled = false;
      if (!c.required) o.required = false;
      const pr: Record<string, unknown> = { type: c.probe.type };
      const q = c.probe;
      if (q.type === "http") {
        if (q.scheme !== "http") pr.scheme = q.scheme;
        if (q.host) pr.host = q.host;
        if (q.allow_remote_host) pr.allow_remote_host = true;
        pr.port = q.port;
        if (q.path !== "/") pr.path = q.path;
        if (q.method !== "GET") pr.method = q.method;
        if (q.headers?.length) pr.headers = q.headers;
        if (!(q.expected_statuses.length === 1 && q.expected_statuses[0]?.from === 200 && q.expected_statuses[0]?.to === 299)) pr.expected_statuses = q.expected_statuses;
        if (!q.tls_verify) pr.tls_verify = false;
        if (q.server_name) pr.server_name = q.server_name;
      } else if (q.type === "tcp") {
        if (q.host) pr.host = q.host;
        if (q.allow_remote_host) pr.allow_remote_host = true;
        pr.port = q.port;
      } else {
        if (q.type === "exec") pr.argv = q.argv;
        else {
          if (q.interpreter !== "sh") pr.interpreter = q.interpreter;
          pr.script = q.script;
        }
        if (!(q.expected_exit_codes.length === 1 && q.expected_exit_codes[0] === 0)) pr.expected_exit_codes = q.expected_exit_codes;
        if (q.show_output) pr.show_output = true;
      }
      o.probe = pr;
      if (c.period_secs !== 10) o.period_secs = c.period_secs;
      if (c.timeout_secs !== 2) o.timeout_secs = c.timeout_secs;
      if (c.initial_delay_secs !== 0) o.initial_delay_secs = c.initial_delay_secs;
      if (c.failure_threshold !== 3) o.failure_threshold = c.failure_threshold;
      if (c.success_threshold !== 1) o.success_threshold = c.success_threshold;
      return o;
    });
  }
  return out;
}

export function toYaml(d: Draft): string {
  const c = compact(toPolicy(d));
  return Object.keys(c).length ? yaml.dump(c, { lineWidth: 120, noRefs: true }) : "# No configured checks; Forge's default checks are on.\n# Add startup, readiness or liveness lists, or default_checks: false.\n";
}

// ------------------------------------------------------------------ YAML parse + shape check

type Kind = "string" | "bool" | "int" | "strings" | "ints";
const CHECK_KEYS: Record<string, Kind | "probe"> = {
  name: "string",
  enabled: "bool",
  required: "bool",
  probe: "probe",
  period_secs: "int",
  timeout_secs: "int",
  initial_delay_secs: "int",
  failure_threshold: "int",
  success_threshold: "int",
};
const PROBE_KEYS: Record<HealthProbeType, Record<string, Kind | "headers" | "statuses">> = {
  http: { type: "string", scheme: "string", host: "string", allow_remote_host: "bool", port: "int", path: "string", method: "string", headers: "headers", expected_statuses: "statuses", tls_verify: "bool", server_name: "string" },
  tcp: { type: "string", host: "string", allow_remote_host: "bool", port: "int" },
  exec: { type: "string", argv: "strings", expected_exit_codes: "ints", show_output: "bool" },
  shell: { type: "string", interpreter: "string", script: "string", expected_exit_codes: "ints", show_output: "bool" },
};
const ENUMS: Record<string, string[]> = { scheme: ["http", "https"], method: ["GET", "HEAD"], interpreter: ["sh", "bash"] };
const REQUIRED_PROBE: Record<HealthProbeType, string[]> = { http: ["port"], tcp: ["port"], exec: ["argv"], shell: ["script"] };

const isObj = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);

function kindOk(k: Kind, v: unknown): boolean {
  const int = (x: unknown) => typeof x === "number" && Number.isInteger(x);
  switch (k) {
    case "string":
      return typeof v === "string";
    case "bool":
      return typeof v === "boolean";
    case "int":
      return int(v) && (v as number) >= 0 && (v as number) <= 4_294_967_295;
    case "strings":
      return Array.isArray(v) && v.every((x) => typeof x === "string");
    case "ints":
      return Array.isArray(v) && v.every(int);
  }
}

const kindText: Record<Kind, string> = { string: "text", bool: "true or false", int: "a whole number", strings: "a list of text", ints: "a list of whole numbers" };

/**
 * Shape problems the server would refuse before validating (unknown keys,
 * wrong types, missing required fields). Value rules (ranges, names) are
 * left to the server's validate.
 */
export function shapeErrors(doc: unknown): string[] {
  const errs: string[] = [];
  if (doc === null || doc === undefined) return errs;
  if (!isObj(doc)) return ["The document must be a mapping with startup, readiness and liveness lists."];
  for (const k of Object.keys(doc)) if (k !== "default_checks" && !PURPOSES.includes(k as HealthPurpose)) errs.push(`\`${k}\`: unknown field (expected default_checks, startup, readiness or liveness)`);
  if (doc.default_checks !== undefined && typeof doc.default_checks !== "boolean") errs.push("`default_checks`: must be true or false");
  for (const purpose of PURPOSES) {
    const list = doc[purpose];
    if (list === undefined || list === null) continue;
    if (!Array.isArray(list)) {
      errs.push(`\`${purpose}\`: must be a list of checks`);
      continue;
    }
    list.forEach((c, i) => {
      const at = `${purpose}[${i}]`;
      if (!isObj(c)) return void errs.push(`\`${at}\`: must be a mapping`);
      for (const [k, v] of Object.entries(c)) {
        const kind = CHECK_KEYS[k];
        if (!kind) errs.push(`\`${at}.${k}\`: unknown field`);
        else if (kind !== "probe" && !kindOk(kind, v)) errs.push(`\`${at}.${k}\`: must be ${kindText[kind]}`);
      }
      if (c.name === undefined) errs.push(`\`${at}.name\`: required`);
      const p = c.probe;
      if (p === undefined) return void errs.push(`\`${at}.probe\`: required`);
      if (!isObj(p)) return void errs.push(`\`${at}.probe\`: must be a mapping`);
      const t = p.type as HealthProbeType;
      if (!(typeof t === "string" && t in PROBE_KEYS)) return void errs.push(`\`${at}.probe.type\`: one of http, tcp, exec, shell`);
      const keys = PROBE_KEYS[t];
      for (const [k, v] of Object.entries(p)) {
        const pa = `${at}.probe.${k}`;
        const kind = keys[k];
        if (!kind) errs.push(`\`${pa}\`: unknown field for a ${t} probe`);
        else if (kind === "headers") {
          if (!Array.isArray(v)) errs.push(`\`${pa}\`: must be a list`);
          else
            v.forEach((h, j) => {
              if (!isObj(h)) return void errs.push(`\`${pa}[${j}]\`: must be a mapping`);
              for (const [hk, hv] of Object.entries(h)) {
                if (!["name", "value", "secret"].includes(hk)) errs.push(`\`${pa}[${j}].${hk}\`: unknown field`);
                else if (typeof hv !== "string") errs.push(`\`${pa}[${j}].${hk}\`: must be text`);
              }
              if (h.name === undefined) errs.push(`\`${pa}[${j}].name\`: required`);
            });
        } else if (kind === "statuses") {
          if (!Array.isArray(v)) errs.push(`\`${pa}\`: must be a list`);
          else
            v.forEach((r, j) => {
              if (!isObj(r)) return void errs.push(`\`${pa}[${j}]\`: must be a mapping with from and to`);
              for (const [rk, rv] of Object.entries(r)) {
                if (!["from", "to"].includes(rk)) errs.push(`\`${pa}[${j}].${rk}\`: unknown field`);
                else if (!(typeof rv === "number" && Number.isInteger(rv) && rv >= 0 && rv <= 65535)) errs.push(`\`${pa}[${j}].${rk}\`: must be a status code`);
              }
              if (r.from === undefined || r.to === undefined) errs.push(`\`${pa}[${j}]\`: needs from and to`);
            });
        } else if (!kindOk(kind, v)) errs.push(`\`${pa}\`: must be ${kindText[kind]}`);
        else if (ENUMS[k] && !ENUMS[k].includes(v as string)) errs.push(`\`${pa}\`: one of ${ENUMS[k].join(", ")}`);
        else if (k === "port" && (v as number) > 65535) errs.push(`\`${pa}\`: between 1 and 65535`);
      }
      for (const k of REQUIRED_PROBE[t]) if (p[k] === undefined) errs.push(`\`${at}.probe.${k}\`: required`);
    });
  }
  return errs;
}

/** Parses the YAML view. `errors` is non-empty when it can't become a draft. */
/** Largest YAML document the editor accepts (the server's limits fit well within it). */
export const MAX_YAML_BYTES = 256 * 1024;

export function parseYaml(text: string): { draft?: Draft; errors: string[] } {
  if (new TextEncoder().encode(text).length > MAX_YAML_BYTES) {
    return { errors: [`The YAML is larger than ${MAX_YAML_BYTES / 1024} KB.`] };
  }
  // Anchors and aliases (&name, *name) can expand into huge documents and
  // aren't part of the policy format. js-yaml records every anchor it
  // defines, so a document that defines one is refused (aliases need one).
  let anchors = 0;
  const listener: yaml.LoadOptions["listener"] = (_event, state) => {
    const map = (state as unknown as { anchorMap?: Record<string, unknown> }).anchorMap;
    anchors = Math.max(anchors, Object.keys(map ?? {}).length);
  };
  let doc: unknown;
  try {
    doc = yaml.load(text, { listener });
    if (anchors > 0) return { errors: ["YAML anchors and aliases (& and *) aren't supported here."] };
  } catch (e) {
    return { errors: [e instanceof Error ? (e.message.split("\n")[0] ?? e.message) : String(e)] };
  }
  const errors = shapeErrors(doc);
  if (errors.length) return { errors };
  return { draft: toDraft((doc ?? {}) as HealthPolicy), errors: [] };
}

// ------------------------------------------------------------------ review diff

export interface DiffLine {
  /** `defaults`: the default checks were turned on or off (`name` is "on" or "off"). */
  purpose: HealthPurpose | "defaults";
  name: string;
  change: "added" | "removed" | "changed";
}

export function diff(before: HealthPolicy, after: HealthPolicy): DiffLine[] {
  const out: DiffLine[] = [];
  if ((before.default_checks !== false) !== (after.default_checks !== false)) out.push({ purpose: "defaults", name: after.default_checks === false ? "off" : "on", change: "changed" });
  for (const purpose of PURPOSES) {
    const a = new Map<string, string>((before[purpose] ?? []).map((c) => [c.name, JSON.stringify(wireCheck(c))]));
    const b = new Map<string, string>((after[purpose] ?? []).map((c) => [c.name, JSON.stringify(wireCheck(c))]));
    for (const [name, v] of b) {
      if (!a.has(name)) out.push({ purpose, name, change: "added" });
      else if (a.get(name) !== v) out.push({ purpose, name, change: "changed" });
    }
    for (const name of a.keys()) if (!b.has(name)) out.push({ purpose, name, change: "removed" });
  }
  return out;
}

export const runsCode = (p: HealthPolicy) => PURPOSES.some((k) => (p[k] ?? []).some((c) => c.probe.type === "exec" || c.probe.type === "shell"));

/** A host other than the workload, `localhost` or loopback needs `allow_remote_host`. */
export function isRemoteHost(host: string | undefined): boolean {
  const h = (host ?? "").trim().toLowerCase();
  if (!h) return false;
  return !(h === "localhost" || h === "::1" || h === "[::1]" || /^127\.\d{1,3}\.\d{1,3}\.\d{1,3}$/.test(h));
}
