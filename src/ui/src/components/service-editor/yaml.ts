import yaml from "js-yaml";

import type { ServiceInput } from "../../api/types";
import { SEALED, looksSecret } from "./sections";

const secs = (s: unknown) => {
  if (typeof s === "number") return s;
  const m = /^(\d+)(ms|s|m|h)?$/.exec(String(s ?? "").trim());
  if (!m) return 0;
  const n = Number(m[1]);
  return m[2] === "m" ? n * 60 : m[2] === "h" ? n * 3600 : m[2] === "ms" ? Math.round(n / 1000) : n;
};
const mib = (s: unknown) => {
  const m = /^(\d+(?:\.\d+)?)\s*([kmg])?i?b?$/i.exec(String(s ?? "").trim());
  if (!m) return 0;
  const n = Number(m[1]);
  const u = (m[2] ?? "m").toLowerCase();
  return Math.round(u === "g" ? n * 1024 : u === "k" ? n / 1024 : n);
};

/** A docker-compose style service document for a container service. */
export function toComposeYaml(v: ServiceInput): string {
  const doc: Record<string, unknown> = { image: v.image };
  if (v.command) doc.command = v.command;
  if (v.entrypoint) doc.entrypoint = v.entrypoint;
  if (v.user) doc.user = v.user;
  if (v.hostname) doc.hostname = v.hostname;
  doc.restart = v.restart ? "unless-stopped" : "no";
  if (v.ports.length) doc.ports = v.ports;
  if (v.volumes.length) doc.volumes = v.volumes;
  if (v.env.length) doc.environment = Object.fromEntries(v.env.map((e) => [e.key, e.secret && e.value === null ? SEALED : (e.value ?? "")]));
  if (v.extra_hosts.length) doc.extra_hosts = v.extra_hosts;
  if (v.labels.length) doc.labels = Object.fromEntries(v.labels);
  if (v.read_only) doc.read_only = true;
  if (v.stop_timeout_secs) doc.stop_grace_period = `${v.stop_timeout_secs}s`;
  if (v.healthcheck)
    doc.healthcheck = {
      test: ["CMD-SHELL", v.healthcheck.command],
      interval: `${v.healthcheck.interval_secs}s`,
      timeout: `${v.healthcheck.timeout_secs}s`,
      retries: v.healthcheck.retries,
      start_period: `${v.healthcheck.start_period_secs}s`,
    };
  if (v.cpu_millis || v.memory_mb)
    doc.deploy = { resources: { limits: { ...(v.cpu_millis ? { cpus: String(v.cpu_millis / 1000) } : {}), ...(v.memory_mb ? { memory: `${v.memory_mb}M` } : {}) } } };
  if (v.log_max_mb) doc.logging = { driver: "json-file", options: { "max-size": `${v.log_max_mb}m` } };
  return yaml.dump(doc, { lineWidth: 120, noRefs: true });
}

const KNOWN = ["image", "command", "entrypoint", "user", "hostname", "restart", "ports", "volumes", "environment", "extra_hosts", "labels", "read_only", "stop_grace_period", "healthcheck", "deploy", "logging"];
const rec = (x: unknown): Record<string, unknown> => (x && typeof x === "object" && !Array.isArray(x) ? (x as Record<string, unknown>) : {});
const str = (x: unknown) => (Array.isArray(x) ? x.join(" ") : x == null ? "" : String(x));
const list = (x: unknown) => (Array.isArray(x) ? x.map(String) : x == null ? [] : [String(x)]);
const pairs = (x: unknown): [string, string][] =>
  Array.isArray(x) ? x.map((l) => String(l).split(/=(.*)/s).slice(0, 2) as [string, string]).map(([k, v]) => [k, v ?? ""]) : Object.entries(rec(x)).map(([k, v]) => [k, v == null ? "" : String(v)]);

/** Applies a compose-style service document; throws with a readable message. */
export function fromComposeYaml(text: string, base: ServiceInput): ServiceInput {
  const loaded = yaml.load(text);
  if (!loaded || typeof loaded !== "object" || Array.isArray(loaded)) throw new Error("Expected a mapping with at least `image:`.");
  const d = loaded as Record<string, unknown>;
  const unknown = Object.keys(d).filter((k) => !KNOWN.includes(k));
  if (unknown.length) throw new Error(`Not supported: ${unknown.join(", ")}`);
  const env: ServiceInput["env"] = pairs(d.environment).map(([k, v]) => {
    const old = base.env.find((x) => x.key === k);
    return v === SEALED && old?.secret ? { key: k, value: null, secret: true } : { key: k, value: v, secret: old?.secret ?? looksSecret(k) };
  });
  const hc = d.healthcheck ? rec(d.healthcheck) : null;
  const test = hc ? (Array.isArray(hc.test) ? hc.test.map(String).filter((t) => t !== "CMD-SHELL" && t !== "CMD").join(" ") : str(hc.test)) : "";
  const limits = rec(rec(rec(d.deploy).resources).limits);
  const maxSize = rec(rec(d.logging).options)["max-size"];
  return {
    ...base,
    runtime: "container",
    image: str(d.image),
    command: str(d.command),
    entrypoint: str(d.entrypoint),
    user: str(d.user),
    hostname: str(d.hostname),
    restart: d.restart ? d.restart !== "no" : true,
    ports: list(d.ports),
    volumes: list(d.volumes),
    env,
    extra_hosts: list(d.extra_hosts),
    labels: pairs(d.labels),
    read_only: !!d.read_only,
    stop_timeout_secs: secs(d.stop_grace_period),
    healthcheck: hc && test ? { command: test, interval_secs: secs(hc.interval) || 30, timeout_secs: secs(hc.timeout) || 5, retries: Number(hc.retries ?? 3) || 3, start_period_secs: secs(hc.start_period) } : null,
    cpu_millis: limits.cpus ? Math.round(Number(limits.cpus) * 1000) || 0 : 0,
    memory_mb: limits.memory ? mib(limits.memory) : 0,
    log_max_mb: maxSize ? mib(maxSize) : 0,
  };
}
