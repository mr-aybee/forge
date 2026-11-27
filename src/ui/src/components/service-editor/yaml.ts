import yaml from "js-yaml";

import type { RestartPolicy, ServiceInput } from "../../api/types";
import { SEALED, looksSecret, policyOf } from "./sections";

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
  const policy = policyOf(v);
  doc.restart = policy === "on-failure" && v.restart_max_attempts ? `on-failure:${v.restart_max_attempts}` : policy;
  if (v.init) doc.init = true;
  if (v.networks.length) doc.networks = v.networks;
  if (v.dns.length) doc.dns = v.dns;
  if (v.cap_add.length) doc.cap_add = v.cap_add;
  if (v.cap_drop.length) doc.cap_drop = v.cap_drop;
  if (v.devices.length) doc.devices = v.devices.map((d) => [d.host, d.container || (d.permissions ? d.host : ""), d.permissions ?? ""].filter(Boolean).join(":"));
  if (v.tmpfs.length) doc.tmpfs = v.tmpfs.map((t) => (t.size_mb ? `${t.target}:size=${t.size_mb}m` : t.target));
  if (v.sysctls.length) doc.sysctls = Object.fromEntries(v.sysctls);
  if (v.ulimits.length) doc.ulimits = Object.fromEntries(v.ulimits.map((u) => [u.name, { soft: u.soft, hard: u.hard }]));
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

const KNOWN = ["image", "command", "entrypoint", "user", "hostname", "restart", "ports", "volumes", "environment", "extra_hosts", "labels", "read_only", "stop_grace_period", "healthcheck", "deploy", "logging", "init", "networks", "dns", "cap_add", "cap_drop", "devices", "tmpfs", "sysctls", "ulimits"];
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
  const restart = d.restart == null ? "unless-stopped" : String(d.restart);
  const rm = /^(no|always|unless-stopped|on-failure)(?::(\d+))?$/.exec(restart);
  if (!rm) throw new Error("restart: use no, always, unless-stopped or on-failure[:N].");
  const policy = rm[1] as RestartPolicy;
  const nets = Array.isArray(d.networks) ? d.networks.map(String) : Object.keys(rec(d.networks));
  const ulimits = Object.entries(rec(d.ulimits)).map(([name, u]) => {
    if (typeof u === "number") return { name, soft: u, hard: u };
    const r = rec(u);
    return { name, soft: Number(r.soft ?? 0), hard: Number(r.hard ?? 0) };
  });
  return {
    ...base,
    restart_policy: policy,
    restart_max_attempts: policy === "on-failure" ? Number(rm[2] ?? 0) : 0,
    init: !!d.init,
    networks: nets,
    dns: list(d.dns),
    cap_add: list(d.cap_add),
    cap_drop: list(d.cap_drop),
    devices: list(d.devices).map((x) => {
      const [host = "", container = "", permissions = ""] = x.split(":");
      return { host, container, permissions };
    }),
    tmpfs: list(d.tmpfs).map((x) => {
      const [target = "", opts = ""] = x.split(/:(.*)/s);
      const size = /size=([^,]+)/.exec(opts)?.[1];
      return { target, size_mb: size ? mib(size) : 0 };
    }),
    sysctls: pairs(d.sysctls),
    ulimits,
    runtime: "container",
    image: str(d.image),
    command: str(d.command),
    entrypoint: str(d.entrypoint),
    user: str(d.user),
    hostname: str(d.hostname),
    restart: policy !== "no",
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
