import { Link, useNavigate, useSearch } from "@tanstack/react-router";
import { ChevronDown, ChevronRight, Loader2, ScrollText, SearchX, ShieldAlert, ShieldCheck } from "lucide-react";
import { useMemo, useState } from "react";

import { useApiKeys } from "../api/accessControl";
import {
  AUDIT_OUTCOMES,
  AUDIT_RANGES,
  type AuditEntry,
  type AuditFilter,
  type AuditHistogram,
  type AuditOutcome,
  type VerifyReport,
  parseAuditSearch,
  useAuditActions,
  useAuditEntries,
  useAuditHistogram,
  useVerifyAudit,
} from "../api/audit";
import { ApiError } from "../api/client";
import { useClusters, useProjects, useServers, useServices, useUsers } from "../api/hooks";
import { nodeName } from "../api/types";
import { errorText, useToast } from "../app/toast";
import { StackedHistogram } from "../components/charts/StackedHistogram";
import { FeedTail, Meta, MetaGrid } from "../components/feed/Feed";
import { CheckDropdown, type CheckOption } from "../components/filters/CheckDropdown";
import { QueryInput } from "../components/filters/QueryInput";
import { TimeRange } from "../components/filters/TimeRange";
import { Button, CopyIcon, Empty, ErrorNote, PageHeader, Skeleton, ago, short, stamp, time } from "../components/ui";

const nf = new Intl.NumberFormat();

// ------------------------------------------------------------------ colours

/** Chip colours per action category (the text before the first `.`). */
const CATEGORY: { tone: string; prefixes: string[] }[] = [
  { tone: "bg-violet-soft text-violet", prefixes: ["auth", "session", "user", "users", "mfa", "passkey", "identity", "token", "device", "account", "login", "apikey", "oauth"] },
  { tone: "bg-sky-soft text-sky", prefixes: ["files", "file", "sftp", "object", "objects", "storage", "upload"] },
  { tone: "bg-teal-soft text-teal", prefixes: ["stack", "stacks", "container", "containers", "app", "apps", "service", "services", "project", "installation", "image", "deploy", "catalog"] },
  { tone: "bg-indigo-soft text-indigo", prefixes: ["server", "servers", "node", "nodes", "agent", "agents", "cluster", "clusters", "environment", "fabric", "enrollment", "tags", "tag"] },
  { tone: "bg-pink-soft text-pink", prefixes: ["remote", "access", "terminal", "ssh", "vault", "secret", "secrets", "connector", "process", "mcp"] },
  { tone: "bg-orange-soft text-orange", prefixes: ["admin", "settings", "role", "roles", "binding", "bindings", "plugin", "plugins", "workspace", "approval", "approvals", "audit", "pki", "certificate", "platform"] },
  { tone: "bg-accent-soft text-accent", prefixes: ["job", "jobs", "site", "sites", "proxy", "dns"] },
];

const category = (action: string) => action.split(".")[0] ?? action;
const categoryTone = (action: string) => CATEGORY.find((k) => k.prefixes.includes(category(action).toLowerCase()))?.tone ?? "bg-panel-2 text-muted";

/** Outcome styling: the chip, the row stripe and the histogram bar. */
const OUTCOME: Record<AuditOutcome, { label: string; short: string; chip: string; stripe: string; bar: string }> = {
  success: { label: "Success", short: "OK", chip: "bg-ok-soft text-ok", stripe: "bg-ok/50", bar: "bg-ok" },
  denied: { label: "Denied", short: "DENY", chip: "bg-bad-soft text-bad", stripe: "bg-bad", bar: "bg-bad" },
  failure: { label: "Failed", short: "FAIL", chip: "bg-warn-soft text-warn", stripe: "bg-warn", bar: "bg-warn" },
};
const SERIES = AUDIT_OUTCOMES.map((o) => ({ key: o, label: OUTCOME[o].label, bar: OUTCOME[o].bar }));

// ------------------------------------------------------------------ names

/** Lookups that turn ids into names; every list is optional (the caller may not be allowed to read it). */
interface Names {
  users: Map<string, string>;
  keys: Map<string, string>;
  nodes: Map<string, string>;
  clusters: Map<string, string>;
  services: Map<string, string>;
  projects: Map<string, string>;
  environments: Map<string, string>;
}

function useNames(): Names {
  const users = useUsers();
  const keys = useApiKeys();
  const servers = useServers();
  const clusters = useClusters();
  const services = useServices();
  const projects = useProjects();
  return useMemo(() => {
    const arr = <T,>(v: T[] | undefined): T[] => (Array.isArray(v) ? v : []);
    return {
      users: new Map(arr(users.data).map((u) => [u.id, u.display_name || u.username || u.email || short(u.id)])),
      keys: new Map(arr(keys.data).map((k) => [k.id, k.name])),
      nodes: new Map(arr(servers.data).map((s) => [s.id, nodeName(s)])),
      clusters: new Map(arr(clusters.data).map((c) => [c.id, c.name])),
      services: new Map(arr(services.data).map((s) => [s.id, s.name])),
      projects: new Map(arr(projects.data).map((p) => [p.id, p.name])),
      environments: new Map(arr(projects.data).flatMap((p) => arr(p.environments).map((e) => [e.id, `${p.name}/${e.name}`] as const))),
    };
  }, [users.data, keys.data, servers.data, clusters.data, services.data, projects.data]);
}

const splitRef = (ref: string): [string, string] => {
  const at = ref.indexOf(":");
  return at < 0 ? [ref, ""] : [ref.slice(0, at), ref.slice(at + 1)];
};

const ACTOR_KIND: Record<string, string> = { user: "user", agent: "agent", token: "token", system: "system" };

interface Actor {
  kind: string;
  name: string;
  /** The API key used, when the call was made with one. */
  key?: string;
}

function describeActor(e: AuditEntry, n: Names): Actor {
  const [kind, rest] = splitRef(e.actor);
  const id = rest.split(":")[0] ?? rest;
  const tokenId = typeof e.details.token_id === "string" ? e.details.token_id : undefined;
  const key = tokenId ? (n.keys.get(tokenId) ?? `key ${short(tokenId)}`) : undefined;
  if (kind === "user") return { kind, name: n.users.get(id) ?? short(id), key };
  if (kind === "agent") return { kind, name: n.nodes.get(id) ?? short(id) };
  if (kind === "system") return { kind, name: rest || "system" };
  return { kind: ACTOR_KIND[kind] ?? kind, name: rest ? short(rest) : e.actor || "—", key };
}

const RESOURCE_KIND: Record<string, string> = {
  server: "node",
  environment: "env",
  audit_log: "audit",
  installation: "install",
  certificate: "cert",
};

function describeResource(e: AuditEntry, n: Names): { kind: string; type: string; id: string; name: string } {
  const [type, id] = splitRef(e.resource);
  const kind = RESOURCE_KIND[type] ?? type;
  const job = typeof e.details.kind === "string" ? e.details.kind : undefined;
  const name =
    (type === "server" && n.nodes.get(id)) ||
    (type === "cluster" && n.clusters.get(id)) ||
    (type === "service" && n.services.get(id)) ||
    (type === "project" && n.projects.get(id)) ||
    (type === "environment" && n.environments.get(id)) ||
    (type === "user" && n.users.get(id)) ||
    (type === "job" && job) ||
    (id.length > 12 ? short(id) : id) ||
    "—";
  return { kind, type, id, name };
}

/** The resource as a link to its page where one exists. */
function ResourceLink({ type, id, name }: { type: string; id: string; name: string }) {
  const cls = "text-accent hover:underline";
  if (type === "server") return <Link to="/nodes/$nodeId" params={{ nodeId: id }} className={cls}>{name}</Link>;
  if (type === "cluster") return <Link to="/clusters/$clusterId" params={{ clusterId: id }} className={cls}>{name}</Link>;
  if (type === "service") return <Link to="/services/$serviceId" params={{ serviceId: id }} className={cls}>{name}</Link>;
  if (type === "project" || type === "environment") return <Link to="/projects" className={cls}>{name}</Link>;
  if (type === "job") return <Link to="/jobs" className={cls}>{name}</Link>;
  if (type === "user") return <Link to="/members" className={cls}>{name}</Link>;
  return <span>{name}</span>;
}

function Avatar({ name, kind }: { name: string; kind: string }) {
  const tone = kind === "user" ? "bg-accent-soft text-accent" : kind === "agent" ? "bg-indigo-soft text-indigo" : "bg-panel-2 text-muted";
  return (
    <span aria-hidden className={`grid size-4 shrink-0 place-items-center rounded-full text-[9px] font-semibold uppercase ${tone}`}>
      {(name.trim()[0] ?? "?").toUpperCase()}
    </span>
  );
}

const show = (v: unknown): string => (v === null || v === undefined ? "null" : typeof v === "string" ? v : JSON.stringify(v));

/** `kind=agent.update · nodes=3`, for the one-line row summary. */
const summary = (d: Record<string, unknown>) =>
  Object.entries(d)
    .filter(([k]) => k !== "token_id" && k !== "elevated")
    .map(([k, v]) => `${k}=${show(v)}`)
    .join(" · ");

// ------------------------------------------------------------------ signature status

type Verified = VerifyReport & { at: number };

function signatureStatus(e: AuditEntry, v: Verified | undefined): { tone: string; text: string } {
  if (!e.signature) return { tone: "text-muted", text: "Not signed" };
  if (!v) return { tone: "text-muted", text: "Signed · not checked yet (run Verify chain)" };
  if (v.ok) return e.seq <= v.entries_checked ? { tone: "text-ok", text: `Verified ${ago(v.at)}` } : { tone: "text-muted", text: "Newer than the last chain check" };
  const bad = v.first_invalid?.[0] ?? 0;
  if (e.seq < bad) return { tone: "text-ok", text: `Verified ${ago(v.at)} (before the break at #${bad})` };
  if (e.seq === bad) return { tone: "text-bad", text: `Fails verification: ${v.first_invalid?.[1] ?? "unknown reason"}` };
  return { tone: "text-warn", text: `Not verified: after the break at #${bad}` };
}

// ------------------------------------------------------------------ page

export function Audit() {
  const search = useSearch({ from: "/audit" });
  const navigate = useNavigate();
  const filter = useMemo(() => parseAuditSearch(search), [search]);
  const setFilter = (f: AuditFilter) =>
    void navigate({
      to: "/audit",
      search: {
        q: f.q?.trim() || undefined,
        action: f.actions?.length ? f.actions.join(",") : undefined,
        actor: f.actor || undefined,
        outcome: f.outcome,
        range: f.range,
        since: f.range ? undefined : f.since,
        until: f.range ? undefined : f.until,
      },
      replace: true,
    });

  const [live, setLive] = useState(false);
  const entries = useAuditEntries(filter, live);
  const histogram = useAuditHistogram(filter, 60, live);
  const actions = useAuditActions();
  const verify = useVerifyAudit();
  const names = useNames();
  const users = useUsers();
  const toast = useToast();

  const forbidden = entries.error instanceof ApiError && entries.error.status === 403;
  const pages = entries.data?.pages ?? [];
  const rows = pages.flatMap((p) => p.entries);
  const total = pages[0]?.total;
  const filtered = !!(filter.q || filter.actions?.length || filter.actor || filter.outcome);

  const actionOptions: CheckOption[] = (actions.data ?? []).map((a) => ({ value: a.action, label: a.action, group: category(a.action), count: a.count, mono: true }));
  const actorOptions: CheckOption[] = [
    { value: "user:", label: "Any user", group: "Type" },
    { value: "agent:", label: "Any node agent", group: "Type" },
    { value: "token:", label: "Any enrollment token", group: "Type" },
    { value: "system:", label: "System", group: "Type" },
    ...(Array.isArray(users.data) ? users.data : []).map((u) => {
      const name = names.users.get(u.id) ?? short(u.id);
      return { value: `user:${u.id}`, label: name, group: "Users", icon: <Avatar name={name} kind="user" /> };
    }),
  ];
  if (filter.actor && !actorOptions.some((o) => o.value === filter.actor)) actorOptions.push({ value: filter.actor, label: filter.actor, group: "Current", mono: true });

  const runVerify = () => verify.mutate(undefined, { onError: (e) => toast(errorText(e), "error") });

  return (
    <>
      <PageHeader
        eyebrow="Operations"
        title="Audit log"
        description="Who did what, and when. Append-only and hash-chained: each entry includes the previous entry's hash and is signed. Newest first."
        actions={
          <Button onClick={runVerify} disabled={verify.isPending || forbidden} title={forbidden ? "The auditor or an admin role is required" : "Recompute every hash and signature from the first entry"}>
            {verify.isPending ? <Loader2 className="size-4 animate-spin" /> : <ShieldCheck className="size-4" />}
            {verify.isPending ? "Verifying…" : "Verify chain"}
          </Button>
        }
      />
      {verify.data && <VerifyResult v={verify.data} />}

      <div className="mb-3 space-y-2">
        <QueryInput
          key={filter.q ?? ""}
          value={filter.q ?? ""}
          onRun={(q) => {
            setLive(false);
            if ((q.trim() || undefined) !== (filter.q?.trim() || undefined)) setFilter({ ...filter, q });
            else {
              void entries.refetch();
              void histogram.refetch();
            }
          }}
          loading={entries.isFetching}
          live={live}
          liveDisabled={forbidden ? "The auditor or an admin role is required" : !entries.isSuccess ? "Waiting for the first page" : undefined}
          onLive={setLive}
          label="Search the audit log"
          placeholder="Text in the action, actor, resource or details  (Enter to run)"
          noun="entries"
          liveEvery="5 seconds"
        />
        <div className="flex flex-wrap items-center gap-2">
          <OutcomeChips filter={filter} onChange={setFilter} histogram={histogram.data} />
          <TimeRange presets={AUDIT_RANGES} value={filter} onChange={(r) => setFilter({ ...filter, range: r.range, since: r.since, until: r.until })} />
          <div className="ml-auto flex min-w-0 flex-wrap items-center justify-end gap-2">
            <CheckDropdown
              label="Action types"
              placeholder="All types"
              noun="action types"
              options={actionOptions}
              value={filter.actions ?? []}
              onChange={(v) => setFilter({ ...filter, actions: v.length ? v : undefined })}
              groupClass={categoryTone}
              width="w-44"
            />
            <CheckDropdown
              label="Actor"
              placeholder="Any actor"
              noun="actors"
              single
              options={actorOptions}
              value={filter.actor ? [filter.actor] : []}
              onChange={(v) => setFilter({ ...filter, actor: v[0] })}
              width="w-40"
            />
            {filtered && (
              <button type="button" className="text-xs text-accent hover:underline" onClick={() => setFilter({ range: filter.range, since: filter.since, until: filter.until })}>
                Clear filters
              </button>
            )}
          </div>
        </div>
      </div>

      {forbidden ? (
        <div className="rounded-lg border border-line bg-panel">
          <Empty icon={<ScrollText className="size-5" />} title="You can't read the audit log">
            The auditor or an admin role is required.
          </Empty>
        </div>
      ) : (
        <>
          <StackedHistogram
            storageKey="forge.audit.chart"
            series={SERIES}
            buckets={(histogram.data?.buckets ?? []).map((b) => ({ t: b.t, counts: { success: b.success, denied: b.denied, failure: b.failure } }))}
            from={histogram.data?.from}
            to={histogram.data?.to}
            loading={histogram.isPending}
            error={histogram.error}
            noun="entries"
            total={total}
            empty={histogram.data === null ? "The chart needs a newer Forge server." : undefined}
          />
          <div className="overflow-hidden rounded-lg border border-line bg-panel">
            {entries.isPending ? (
              <Skeleton rows={10} />
            ) : entries.isError ? (
              <ErrorNote error={entries.error} />
            ) : rows.length === 0 ? (
              filtered || filter.range !== "all" ? (
                <Empty icon={<SearchX className="size-5" />} title="No entries match" action={<Button onClick={() => setFilter({ range: "all" })}>Clear filters and show all time</Button>}>
                  Nothing in this window matches. Widen the time range or remove filters.
                </Empty>
              ) : (
                <Empty icon={<ScrollText className="size-5" />} title="The audit log is empty">
                  Every sign-in and change is recorded here as it happens.
                </Empty>
              )
            ) : (
              <>
                <ul role="list" className="divide-y divide-line/70">
                  {rows.map((e) => (
                    <Row key={e.seq} e={e} names={names} verified={verify.data} onFind={(q) => setFilter({ ...filter, q })} />
                  ))}
                </ul>
                <FeedTail hasMore={!!entries.hasNextPage} loading={entries.isFetchingNextPage} more={() => void entries.fetchNextPage()} error={entries.isFetchNextPageError} noun="entries" end="End of audit log">
                  {total !== undefined ? `${nf.format(rows.length)} of ${nf.format(total)}` : nf.format(rows.length)}
                </FeedTail>
              </>
            )}
          </div>
        </>
      )}
    </>
  );
}

function VerifyResult({ v }: { v: Verified }) {
  const bad = v.first_invalid;
  return (
    <div role="status" className={`mb-3 flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 rounded-lg border px-3 py-1.5 text-xs ${v.ok ? "border-ok/40 bg-ok-soft text-ok" : "border-bad/40 bg-bad-soft text-bad"}`}>
      {v.ok ? <ShieldCheck className="size-3.5 shrink-0" /> : <ShieldAlert className="size-3.5 shrink-0" />}
      <span className="font-semibold">{v.ok ? "Chain intact" : `Chain broken at entry #${bad?.[0] ?? "?"}`}</span>
      {v.ok ? (
        <>
          <span>{nf.format(v.entries_checked)} entries verified</span>
          {v.head_hash && (
            <span className="flex min-w-0 items-center gap-1">
              head <span className="min-w-0 truncate font-mono text-[11px] select-text" title={v.head_hash}>{v.head_hash.slice(0, 16)}…</span>
              <CopyIcon text={v.head_hash} label="Copy head hash" />
            </span>
          )}
        </>
      ) : (
        <span className="min-w-0">
          {bad?.[1] ?? "Unknown reason"}. {nf.format(v.entries_checked)} entries before it verify; entries from #{bad?.[0] ?? "?"} on cannot be trusted.
        </span>
      )}
      <span className="ml-auto opacity-80" title={time(v.at)}>
        checked {ago(v.at)}
      </span>
    </div>
  );
}

function OutcomeChips({ filter, onChange, histogram }: { filter: AuditFilter; onChange: (f: AuditFilter) => void; histogram?: AuditHistogram | null }) {
  const counts = useMemo(() => {
    if (!histogram) return undefined;
    const c = { success: 0, denied: 0, failure: 0 };
    for (const b of histogram.buckets) for (const o of AUDIT_OUTCOMES) c[o] += b[o];
    return c;
  }, [histogram]);
  const all = counts ? counts.success + counts.denied + counts.failure : undefined;
  const chip = (active: boolean) =>
    `press inline-flex h-7 items-center gap-1.5 border-l border-line px-2.5 text-xs whitespace-nowrap transition first:border-l-0 focus-visible:relative focus-visible:outline-2 focus-visible:outline-accent ${active ? "bg-accent-soft text-fg" : "text-muted hover:bg-panel-2 hover:text-fg"}`;
  const n = (v: number | undefined) => <span className="font-mono text-[11px] text-faint tabular-nums">{v === undefined ? "—" : nf.format(v)}</span>;
  return (
    <div role="group" aria-label="Outcome" className="inline-flex overflow-hidden rounded-full border border-line bg-panel">
      <button type="button" aria-pressed={!filter.outcome} onClick={() => onChange({ ...filter, outcome: undefined })} className={chip(!filter.outcome)} title="Every outcome">
        All {n(all)}
      </button>
      {AUDIT_OUTCOMES.map((o) => {
        const active = filter.outcome === o;
        return (
          <button key={o} type="button" aria-pressed={active} onClick={() => onChange({ ...filter, outcome: active ? undefined : o })} className={chip(active)} title={`${active ? "Show every outcome" : `Show only ${OUTCOME[o].label.toLowerCase()}`}`}>
            <span aria-hidden className={`size-2 rounded-full ${OUTCOME[o].bar}`} />
            {OUTCOME[o].label}
            {/* With an outcome chosen, the histogram counts only that outcome. */}
            {n(filter.outcome && !active ? undefined : counts?.[o])}
          </button>
        );
      })}
    </div>
  );
}

function Row({ e, names, verified, onFind }: { e: AuditEntry; names: Names; verified?: Verified; onFind: (q: string) => void }) {
  const [open, setOpen] = useState(false);
  const oc = OUTCOME[e.outcome];
  const actor = describeActor(e, names);
  const res = describeResource(e, names);
  const text = summary(e.details);
  return (
    <li className="relative">
      <span aria-hidden className={`absolute inset-y-0 left-0 ${e.outcome === "success" ? "w-0.5" : "w-1"} ${oc.stripe}`} />
      <button
        type="button"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
        className="flex w-full min-w-0 items-center gap-2 py-1 pr-3 pl-2.5 text-left text-[13px] hover:bg-panel-2 focus-visible:bg-panel-2"
      >
        {open ? <ChevronDown className="size-3.5 shrink-0 text-faint" /> : <ChevronRight className="size-3.5 shrink-0 text-faint" />}
        <time className="shrink-0 font-mono text-[11px] text-faint tabular-nums" title={`${time(e.at)} · ${ago(e.at)}`} dateTime={new Date(e.at || 0).toISOString()}>
          {stamp(e.at)}
        </time>
        <span className={`w-9 shrink-0 rounded px-1 text-center font-mono text-[10px] leading-4 ${oc.chip}`} title={oc.label}>
          {oc.short}
        </span>
        <span className="flex w-40 min-w-0 shrink-0" title={e.action}>
          <span className={`min-w-0 truncate rounded px-1 font-mono text-[11px] leading-4 ${categoryTone(e.action)}`}>{e.action || "—"}</span>
        </span>
        <span className="flex w-36 min-w-0 shrink-0 items-center gap-1.5 text-xs" title={`${actor.name}${actor.key ? ` with ${actor.key}` : ""} (${e.actor})`}>
          <Avatar name={actor.name} kind={actor.kind} />
          <span className={`min-w-0 truncate ${actor.kind === "system" ? "text-muted" : ""}`}>{actor.name}</span>
          {actor.key && <span className="shrink-0 rounded bg-panel-2 px-1 text-[10px] leading-4 text-muted">key</span>}
        </span>
        <span className="hidden w-48 min-w-0 shrink-0 items-center gap-1 text-xs sm:flex" title={e.resource}>
          <span className="shrink-0 rounded bg-panel-2 px-1 text-[10px] leading-4 text-muted">{res.kind || "—"}</span>
          <span className="min-w-0 truncate text-muted">{res.name}</span>
        </span>
        <span className="hidden min-w-0 flex-1 font-mono text-[11px] text-faint [overflow-wrap:anywhere] md:line-clamp-2">{text}</span>
      </button>
      {open && <Detail e={e} actor={actor} res={res} verified={verified} onFind={onFind} />}
    </li>
  );
}

function Detail({
  e,
  actor,
  res,
  verified,
  onFind,
}: {
  e: AuditEntry;
  actor: Actor;
  res: ReturnType<typeof describeResource>;
  verified?: Verified;
  onFind: (q: string) => void;
}) {
  const props = Object.entries(e.details);
  const sig = signatureStatus(e, verified);
  const actorId = splitRef(e.actor)[1].split(":")[0] ?? "";
  return (
    <div className="space-y-2 border-t border-line/70 bg-panel-2/40 py-2 pr-3 pl-8 text-[13px]">
      <MetaGrid>
        <Meta label="Sequence" value={`#${e.seq}`} mono copy={String(e.seq)} />
        <Meta label="Time" value={`${time(e.at)} · ${ago(e.at)}`} />
        <Meta label="Outcome">
          <span className={`rounded px-1 font-mono text-[10px] leading-4 ${OUTCOME[e.outcome].chip}`}>{OUTCOME[e.outcome].label}</span>
        </Meta>
        <Meta label="Action" value={e.action} mono copy={e.action} />
        <Meta label="Actor">
          {actor.kind === "user" ? (
            <Link to="/members" className="text-accent hover:underline">
              {actor.name}
            </Link>
          ) : actor.kind === "agent" && actorId ? (
            <Link to="/nodes/$nodeId" params={{ nodeId: actorId }} className="text-accent hover:underline">
              {actor.name}
            </Link>
          ) : (
            actor.name
          )}
          <span className="ml-1.5 text-faint">{actor.kind}</span>
        </Meta>
        <Meta label="Actor id" value={e.actor || "—"} mono copy={e.actor} />
        {actor.key && <Meta label="API key" value={`${actor.key}${e.details.elevated === true ? " · elevated" : ""}`} />}
        <Meta label="Resource">
          <span className="mr-1.5 rounded bg-panel-2 px-1 text-[10px] leading-4 text-muted">{res.kind || "—"}</span>
          <ResourceLink type={res.type} id={res.id} name={res.name} />
        </Meta>
        <Meta label="Resource id" value={e.resource || "—"} mono copy={e.resource} />
        {e.workspace_id && <Meta label="Workspace" value={e.workspace_id} mono copy={e.workspace_id} />}
        <Meta label="Entry id" value={e.id || "—"} mono copy={e.id} />
        <Meta label="Hash" value={e.hash || "—"} mono copy={e.hash} />
        <Meta label="Previous hash" value={e.prev_hash || "—"} mono copy={e.prev_hash} />
        <Meta label="Signature">
          <span className={sig.tone}>{sig.text}</span>
        </Meta>
        {e.signature && <Meta label="Signature value" value={e.signature} mono copy={e.signature} />}
        <div className="contents">
          <dt className="col-span-2 mt-1.5 border-t border-line/60 pt-1.5 text-[10px] font-semibold tracking-wide text-faint uppercase">Request details</dt>
        </div>
        {props.length === 0 && <Meta label="—" value="No request details" />}
        {props.map(([k, v]) => (
          <Meta key={k} label={k}>
            <span className="group inline-flex max-w-full min-w-0 items-center gap-1.5">
              <span className="min-w-0 truncate font-mono text-[11px] select-text" title={show(v)}>
                {show(v)}
              </span>
              {(typeof v === "string" || typeof v === "number") && String(v) && (
                <button
                  type="button"
                  className="shrink-0 rounded px-1 text-[11px] text-accent opacity-60 group-hover:opacity-100 hover:bg-accent-soft focus-visible:opacity-100"
                  title={`Find entries containing ${show(v)}`}
                  onClick={() => onFind(String(v))}
                >
                  Find
                </button>
              )}
              <CopyIcon text={show(v)} label={`Copy ${k}`} />
            </span>
          </Meta>
        ))}
      </MetaGrid>
    </div>
  );
}
