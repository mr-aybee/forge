import { FilterBar as SharedFilterBar } from "../components/filters/FilterBar";
import { Link, useNavigate, useSearch } from "@tanstack/react-router";
import { ChevronDown, ChevronRight, CircleOff, KeyRound, SearchX } from "lucide-react";
import { type ReactNode, useMemo, useState } from "react";

import { analyticsUnavailable } from "../api/analytics";
import { useServices, useServers } from "../api/hooks";
import { LOG_LEVELS, type LogEvent, type LogFilter, type LogHistogram, type LogLevel, parseLevels, parseList, useLogEvents, useLogHistogram, useLogTail } from "../api/logs";
import { type Service, nodeName, type Server } from "../api/types";
import { type TreeGroup, type TreeSelection, TreeMultiSelect } from "../components/TreeMultiSelect";
import { StackedHistogram } from "../components/charts/StackedHistogram";
import { FeedTail, Meta, MetaGrid } from "../components/feed/Feed";
import { QueryInput } from "../components/filters/QueryInput";
import { Button, CopyIcon, Empty, ErrorNote, PageHeader, Skeleton, stamp, time } from "../components/ui";

/** Severity styling, used for the chip, the row stripe and the histogram. */
const LEVEL: Record<LogLevel, { short: string; chip: string; stripe: string; bar: string }> = {
  Verbose: { short: "VRB", chip: "bg-panel-2 text-faint", stripe: "bg-line-strong", bar: "bg-faint/50" },
  Debug: { short: "DBG", chip: "bg-panel-2 text-muted", stripe: "bg-faint", bar: "bg-faint" },
  Information: { short: "INF", chip: "bg-accent-soft text-accent", stripe: "bg-accent/60", bar: "bg-accent" },
  Warning: { short: "WRN", chip: "bg-warn-soft text-warn", stripe: "bg-warn", bar: "bg-warn" },
  Error: { short: "ERR", chip: "bg-bad-soft text-bad", stripe: "bg-bad", bar: "bg-bad" },
  Fatal: { short: "FTL", chip: "bg-fatal text-white font-bold", stripe: "bg-fatal", bar: "bg-fatal" },
};

const nf = new Intl.NumberFormat();

const LEVEL_SERIES = LOG_LEVELS.map((l) => ({ key: l, label: l, bar: LEVEL[l].bar }));

const show = (v: unknown): string => (v === null || v === undefined ? "null" : typeof v === "string" ? v : JSON.stringify(v));

/** A query token for `name = value`, quoted unless it is a bare word. */
function token(name: string, v: unknown): string {
  const s = show(v);
  return `${name}=${/^[\w.-]+$/.test(s) ? s : `"${s.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`}`;
}

/** Renders the message template with property values highlighted; falls back to the rendered message. */
function Message({ e }: { e: LogEvent }) {
  const tpl = e.message_template;
  if (!tpl || !/\{[@$]?[\w.]+(?:[,:][^}]*)?\}/.test(tpl)) return <>{e.message || "—"}</>;
  const out: ReactNode[] = [];
  const re = /\{\{|\}\}|\{([@$]?)([\w.]+)(?:[,:][^}]*)?\}/g;
  let last = 0;
  let m: RegExpExecArray | null;
  while ((m = re.exec(tpl))) {
    if (m.index > last) out.push(tpl.slice(last, m.index));
    const name = m[2];
    if (m[0] === "{{" || m[0] === "}}") out.push(m[0].charAt(0));
    else if (name !== undefined && name in e.properties) {
      out.push(
        <span key={m.index} className="text-accent">
          {show(e.properties[name])}
        </span>,
      );
    } else out.push(m[0]);
    last = re.lastIndex;
  }
  out.push(tpl.slice(last));
  return <>{out}</>;
}

export function Logs() {
  const search = useSearch({ from: "/logs" });
  const navigate = useNavigate();
  const filter: LogFilter = useMemo(() => {
    const services = parseList(search.services);
    const instances = parseList(search.nodes)?.filter((x) => x.includes(":") && !services?.includes(x.split(":")[0]!));
    return {
      services,
      instances: instances?.length ? instances : undefined,
      nodes: parseList(search.node),
      container: search.container,
      stack: search.stack,
      source: search.source,
      levels: parseLevels(search.levels),
      q: search.q,
    };
  }, [search.services, search.nodes, search.node, search.container, search.stack, search.source, search.levels, search.q]);
  const setFilter = (next: LogFilter) =>
    void navigate({
      to: "/logs",
      search: {
        services: next.services?.length ? next.services.join(",") : undefined,
        nodes: next.instances?.length ? next.instances.join(",") : undefined,
        node: next.nodes?.length ? next.nodes.join(",") : undefined,
        container: next.container || undefined,
        stack: next.stack || undefined,
        source: next.source,
        levels: next.levels?.length ? next.levels.join(",") : undefined,
        q: next.q?.trim() || undefined,
      },
      replace: true,
    });

  const [live, setLive] = useState(false);
  const apps = useServices();
  const servers = useServers();
  const groups = useMemo(() => serviceTree(apps.data, servers.data), [apps.data, servers.data]);
  const events = useLogEvents(filter);
  const histogram = useLogHistogram(filter, live);
  const tail = useLogTail(filter, live && events.isSuccess);

  const off = analyticsUnavailable(events.error) ?? analyticsUnavailable(histogram.error);
  const pages = events.data?.pages ?? [];
  const rows = pages.flatMap((p) => p.events.map((e) => ({ e, fresh: !!p.arrived })));

  const appendToken = (t: string) => setFilter({ ...filter, q: [filter.q?.trim(), t].filter(Boolean).join(" ") });

  return (
    <>
      <PageHeader
        eyebrow="Operations"
        title="Logs"
        description="Structured events sent by your services (Seq / CLEF) and container output collected by the nodes. Newest first."
      />
      <FilterBar
        filter={filter}
        onChange={setFilter}
        live={live}
        liveDisabled={off ? "Analytics is off" : !events.isSuccess ? "Waiting for the first page" : undefined}
        onLive={setLive}
        onRun={(q) => {
          // Run is a one-off load, so it ends live mode. A changed query
          // applies through the URL; the same query reloads.
          setLive(false);
          if ((q.trim() || undefined) !== (filter.q?.trim() || undefined)) setFilter({ ...filter, q });
          else {
            void events.refetch();
            void histogram.refetch();
          }
        }}
        loading={events.isFetching}
        groups={groups}
        histogram={histogram.data}
      >
        <SourceFilters filter={filter} onChange={setFilter} servers={servers.data} seen={rows.map((r) => r.e)} />
      </FilterBar>
      {off ? (
        <div className="rounded-lg border border-line bg-panel">
          <Empty icon={<CircleOff className="size-5" />} title="Log analytics is not available">
            {off} Logs are stored in the analytics database. Turn analytics on in <Link to="/settings" className="text-accent hover:underline">Settings</Link> to collect and search
            application events.
          </Empty>
        </div>
      ) : (
        <>
          <StackedHistogram
            storageKey="forge.logs.chart"
            series={LEVEL_SERIES}
            buckets={histogram.data?.buckets ?? []}
            from={histogram.data?.from}
            to={histogram.data?.to}
            loading={histogram.isPending}
            error={histogram.error && !analyticsUnavailable(histogram.error) ? histogram.error : undefined}
          />
          <div className="overflow-hidden rounded-lg border border-line bg-panel">
            {tail.error && !analyticsUnavailable(tail.error) ? <ErrorNote error={tail.error} /> : null}
            {events.isPending ? (
              <Skeleton rows={10} />
            ) : events.isError ? (
              <ErrorNote error={events.error} />
            ) : rows.length === 0 ? (
              filter.q || filter.levels || filter.services || filter.instances || filter.nodes || filter.container || filter.stack || filter.source ? (
                <Empty icon={<SearchX className="size-5" />} title="No events match" action={<Button onClick={() => setFilter({})}>Clear filters</Button>}>
                  Nothing in the log matches this filter. Widen the levels or remove search terms.
                </Empty>
              ) : (
                <Empty
                  icon={<KeyRound className="size-5" />}
                  title="No log events yet"
                  action={
                    <Link to="/services" className="text-accent hover:underline">
                      Open a service to create an ingest key
                    </Link>
                  }
                >
                  Services send events with a log ingest key (Serilog <code className="font-mono text-xs">WriteTo.Seq</code> or CLEF over HTTP). Create a key on the service's page under
                  <b> Log ingest keys</b>.
                </Empty>
              )
            ) : (
              <>
                <ul role="list" className="divide-y divide-line/70">
                  {rows.map(({ e, fresh }) => (
                    <Row key={e.id} e={e} fresh={fresh} onToken={appendToken} />
                  ))}
                </ul>
                <FeedTail hasMore={!!events.hasNextPage} loading={events.isFetchingNextPage} more={() => void events.fetchNextPage()} error={events.isFetchNextPageError} />
              </>
            )}
          </div>
        </>
      )}
    </>
  );
}

/** Services → the nodes they run on (one entry per node; active before standby). */
function serviceTree(apps: Service[] | undefined, servers: Server[] | undefined): TreeGroup[] {
  const names = new Map((Array.isArray(servers) ? servers : []).map((s) => [s.id, nodeName(s)]));
  return (Array.isArray(apps) ? apps : [])
    .map((a) => {
      const seen = new Set<string>();
      const children = (Array.isArray(a.instances) ? a.instances : [])
        .filter((i) => i?.server_id && !seen.has(i.server_id) && seen.add(i.server_id))
        .sort((x, y) => (x.role === y.role ? 0 : x.role === "active" ? -1 : 1))
        .map((i) => ({ id: i.server_id, label: names.get(i.server_id) ?? i.server_id.slice(0, 8), badge: i.role === "standby" ? "standby" : undefined }));
      return { id: a.id, label: a.name || a.id.slice(0, 8), children };
    })
    .sort((x, y) => x.label.localeCompare(y.label));
}

const toSelection = (f: LogFilter): TreeSelection => ({
  groups: f.services ?? [],
  leaves: (f.instances ?? []).map((x) => {
    const at = x.indexOf(":");
    return { group: x.slice(0, at), leaf: x.slice(at + 1) };
  }),
});

function FilterBar({
  filter,
  onChange,
  onRun,
  loading,
  live,
  liveDisabled,
  onLive,
  groups,
  histogram,
  children,
}: {
  filter: LogFilter;
  onChange: (f: LogFilter) => void;
  onRun: (q: string) => void;
  loading: boolean;
  live: boolean;
  liveDisabled?: string;
  onLive: (on: boolean) => void;
  groups: TreeGroup[];
  histogram?: LogHistogram;
  children?: ReactNode;
}) {
  // Row 1: the query across the full width with a Run button. Row 2: the
  // levels as one joined group, the dropdowns on the right. Every control is
  // h-7. The source pills render through the shared bar, whose wrapper is
  // dissolved so they sit in this row. The query input is keyed on the
  // applied query, so a URL change resets the draft.
  return (
    <div className="mb-3 space-y-2">
      <QueryInput
        key={filter.q ?? ""}
        value={filter.q ?? ""}
        onRun={onRun}
        loading={loading}
        live={live}
        liveDisabled={liveDisabled}
        onLive={onLive}
        label="Search events"
        placeholder='Text, or Property="value"  (Enter to run)'
      />
      <div className="flex flex-wrap items-center gap-2">
        <LevelChips filter={filter} onChange={onChange} histogram={histogram} />
        <div className="ml-auto flex flex-wrap items-center justify-end gap-2">
          <div className="w-52 min-w-0 [&_button[aria-expanded]]:h-7 [&_button[aria-expanded]]:rounded-full [&_button[aria-expanded]]:text-xs">
            <TreeMultiSelect
              label="Service"
              noun="services"
              placeholder="All services"
              groups={groups}
              value={toSelection(filter)}
              onChange={(sel) =>
                onChange({
                  ...filter,
                  services: sel.groups.length ? sel.groups : undefined,
                  instances: sel.leaves.length ? sel.leaves.map((l) => `${l.group}:${l.leaf}`) : undefined,
                })
              }
            />
          </div>
          <div className="contents [&>div]:contents">{children}</div>
        </div>
      </div>
    </div>
  );
}

function LevelChips({ filter, onChange, histogram }: { filter: LogFilter; onChange: (f: LogFilter) => void; histogram?: LogHistogram }) {
  const counts = useMemo(() => {
    const c: Partial<Record<LogLevel, number>> = {};
    for (const b of histogram?.buckets ?? []) for (const l of LOG_LEVELS) c[l] = (c[l] ?? 0) + (b.counts?.[l] ?? 0);
    return c;
  }, [histogram]);
  const on = new Set(filter.levels ?? []);
  const toggle = (l: LogLevel) => {
    const next = new Set(on);
    if (next.has(l)) next.delete(l);
    else next.add(l);
    onChange({ ...filter, levels: LOG_LEVELS.filter((x) => next.has(x)) });
  };
  return (
    <div role="group" aria-label="Levels" className="flex flex-wrap items-center">
      <div className="inline-flex overflow-hidden rounded-full border border-line bg-panel">
      {LOG_LEVELS.map((l) => {
        const active = on.has(l);
        const n = histogram ? counts[l] : undefined;
        return (
          <button
            key={l}
            type="button"
            aria-pressed={active}
            onClick={() => toggle(l)}
            title={`${active ? "Hide" : "Show only"} ${l}${n !== undefined ? ` (${nf.format(n)} in the last window)` : ""}`}
            className={`press inline-flex h-7 items-center gap-1.5 border-l border-line px-2.5 text-xs transition first:border-l-0 focus-visible:relative focus-visible:outline-2 focus-visible:outline-accent ${active ? "bg-accent-soft text-fg" : "text-muted hover:bg-panel-2 hover:text-fg"}`}
          >
            <span aria-hidden className={`size-2 rounded-full ${LEVEL[l].bar}`} />
            {l}
            <span className="font-mono text-[11px] text-faint tabular-nums">{n === undefined ? "—" : nf.format(n)}</span>
          </button>
        );
      })}
      </div>
      {on.size > 0 && (
        <button type="button" className="ml-2 text-xs text-accent hover:underline" onClick={() => onChange({ ...filter, levels: undefined })}>
          All levels
        </button>
      )}
    </div>
  );
}

/** "service on node" or "container on node", for the row tooltip. */
const origin = (e: LogEvent) => [e.service ?? e.container_name ?? "—", e.service && e.container_name ? `(${e.container_name})` : "", e.node ? `on ${e.node}` : "", e.stream ?? ""].filter(Boolean).join(" ");

/** Source, node, container and stack pills; options for container and stack come from the events loaded. */
function SourceFilters({ filter, onChange, servers, seen }: { filter: LogFilter; onChange: (f: LogFilter) => void; servers: Server[] | undefined; seen: LogEvent[] }) {
  const uniq = (xs: (string | null)[], cur?: string) => [...new Set([...xs, cur ?? null].filter((x): x is string => !!x))].sort();
  return (
    <SharedFilterBar
      filters={[
        {
          id: "source",
          label: "Source",
          options: [
            { value: "ingest", label: "Sent by services" },
            { value: "container", label: "Container output" },
          ],
          value: filter.source ? [filter.source] : [],
          onChange: (v) => onChange({ ...filter, source: v[0] === "ingest" || v[0] === "container" ? v[0] : undefined }),
        },
        {
          id: "node",
          label: "Node",
          multi: true,
          options: (Array.isArray(servers) ? servers : []).map((s) => ({ value: s.id, label: nodeName(s) })),
          value: filter.nodes ?? [],
          onChange: (v) => onChange({ ...filter, nodes: v.length ? v : undefined }),
        },
        {
          id: "container",
          label: "Container",
          options: uniq(seen.map((e) => e.container_name), filter.container).map((c) => ({ value: c, label: c })),
          value: filter.container ? [filter.container] : [],
          onChange: (v) => onChange({ ...filter, container: v[0] }),
        },
        {
          id: "stack",
          label: "Stack",
          options: uniq(seen.map((e) => e.stack), filter.stack).map((c) => ({ value: c, label: c })),
          value: filter.stack ? [filter.stack] : [],
          onChange: (v) => onChange({ ...filter, stack: v[0] }),
        },
      ]}
      onClear={() => onChange({ ...filter, source: undefined, nodes: undefined, container: undefined, stack: undefined })}
    />
  );
}

/**
 * A container name without what the row already says: the compose stack
 * prefix (`forge-dev-nodes-node-2-1` in stack `forge-dev-nodes` → `node-2`)
 * and the replica suffix. The full name stays in the tooltip and details.
 */
const shortContainer = (e: LogEvent) => {
  let n = e.container_name ?? "—";
  if (e.stack && n.startsWith(`${e.stack}-`) && n.length > e.stack.length + 1) n = n.slice(e.stack.length + 1);
  return n.replace(/-\d+$/, "") || n;
};

function Row({ e, fresh, onToken }: { e: LogEvent; fresh: boolean; onToken: (t: string) => void }) {
  const [open, setOpen] = useState(false);
  const lv = LEVEL[e.level];
  const fatal = e.level === "Fatal";
  return (
    <li className={`relative ${fresh ? "log-fresh" : ""} ${fatal ? "bg-fatal-soft" : ""}`}>
      <span aria-hidden className={`absolute inset-y-0 left-0 ${fatal ? "w-1" : "w-0.5"} ${lv.stripe}`} />
      <button
        type="button"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
        className="flex w-full min-w-0 items-baseline gap-2 py-1 pr-3 pl-2.5 text-left text-[13px] hover:bg-panel-2 focus-visible:bg-panel-2"
      >
        {open ? <ChevronDown className="size-3.5 shrink-0 self-center text-faint" /> : <ChevronRight className="size-3.5 shrink-0 self-center text-faint" />}
        <time className="shrink-0 font-mono text-[11px] text-faint tabular-nums" title={time(e.ts)} dateTime={new Date(e.ts || 0).toISOString()}>
          {stamp(e.ts)}
        </time>
        <span className={`w-9 shrink-0 rounded px-1 text-center font-mono text-[10px] leading-4 ${lv.chip}`} title={e.level}>
          {lv.short}
        </span>
        <span className={`w-8 shrink-0 rounded px-1 text-center text-[10px] leading-4 ${e.source === "container" ? "bg-accent-soft text-accent" : "bg-panel-2 text-muted"}`} title={e.source === "container" ? "Collected from the container's output" : "Sent by the service"}>
          {e.source === "container" ? "ctr" : "app"}
        </span>
        <span className="flex max-w-[16rem] shrink-0 min-w-0 items-baseline gap-1 text-xs" title={origin(e)}>
          {e.service ? (
            <span className="min-w-0 shrink truncate text-muted">{e.service}</span>
          ) : (
            <span className="min-w-0 shrink truncate font-mono text-[11px] text-muted">{shortContainer(e)}</span>
          )}
          {e.node && <span className="min-w-0 shrink-[3] truncate font-mono text-[11px] text-faint">@{e.node}</span>}
          {e.stream === "stderr" && <span className="shrink-0 font-mono text-[10px] text-bad">err</span>}
        </span>
        <span className={`line-clamp-2 min-w-0 flex-1 [overflow-wrap:anywhere] ${fatal ? "font-semibold" : ""}`}>
          <Message e={e} />
          {e.exception && <span className="ml-2 text-xs text-bad">· exception</span>}
        </span>
      </button>
      {open && <Detail e={e} onToken={onToken} />}
    </li>
  );
}

function Detail({ e, onToken }: { e: LogEvent; onToken: (t: string) => void }) {
  const props = Object.entries(e.properties ?? {});
  return (
    <div className="space-y-2 border-t border-line/70 bg-panel-2/40 py-2 pr-3 pl-8 text-[13px]">
      <p className="break-words whitespace-pre-wrap select-text">{e.message || "—"}</p>
      {e.exception && (
        <div className="relative">
          <pre className="max-h-80 overflow-auto rounded-md border border-line bg-term p-2 pr-8 font-mono text-[11px] leading-relaxed text-term-fg select-text">{e.exception}</pre>
          <CopyIcon text={e.exception} label="Copy exception" className="absolute top-1.5 right-1.5" />
        </div>
      )}
      <MetaGrid>
        <Meta label="Time" value={time(e.ts)} />
        <Meta label="Service">
          {e.service_id ? (
            <Link to="/services/$serviceId" params={{ serviceId: e.service_id }} className="text-accent hover:underline">
              {e.service || e.service_id.slice(0, 8)}
            </Link>
          ) : (
            "—"
          )}
        </Meta>
        <Meta label="Source" value={e.source === "container" ? `container${e.stream ? ` · ${e.stream}` : ""}` : "ingest"} />
        {e.container_name && (
          <Meta label="Container">
            {e.server_id ? (
              <Link to="/nodes/$nodeId/containers/$containerId" params={{ nodeId: e.server_id, containerId: e.container_name }} className="font-mono text-accent hover:underline">
                {e.container_name}
              </Link>
            ) : (
              <span className="font-mono">{e.container_name}</span>
            )}
          </Meta>
        )}
        {e.container_id && <Meta label="Container id" value={e.container_id} mono copy={e.container_id} />}
        {e.stack && <Meta label="Stack" value={e.stack} />}
        <Meta label="Node">
          {e.server_id ? (
            <Link to="/nodes/$nodeId" params={{ nodeId: e.server_id }} className="text-accent hover:underline">
              {e.node || e.server_id.slice(0, 8)}
            </Link>
          ) : (
            (e.node ?? "—")
          )}
        </Meta>
        <Meta label="Event type" value={e.event_type || "—"} mono copy={e.event_type} />
        <Meta label="Trace" value={e.trace_id ?? "—"} mono copy={e.trace_id} />
        <Meta label="Span" value={e.span_id ?? "—"} mono copy={e.span_id} />
        <Meta label="Ingested" value={time(e.ingest_ts)} />
      </MetaGrid>
      {props.length > 0 && (
        <table className="w-full table-fixed text-xs">
          <tbody>
            {props.map(([k, v]) => (
              <tr key={k} className="group border-t border-line/60 align-top">
                <td className="w-48 truncate py-1 pr-3 font-mono text-[11px] text-muted" title={k}>
                  {k}
                </td>
                <td className="py-1 font-mono text-[11px] break-all whitespace-pre-wrap select-text">{show(v)}</td>
                <td className="w-14 py-0.5 text-right whitespace-nowrap">
                  <button type="button" className="rounded px-1.5 py-0.5 text-accent opacity-60 group-hover:opacity-100 hover:bg-accent-soft focus-visible:opacity-100" title={`Find ${token(k, v)}`} onClick={() => onToken(token(k, v))}>
                    Find
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}
