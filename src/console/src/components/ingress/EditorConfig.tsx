import { Loader2, RefreshCw } from "lucide-react";
import { useMemo, useState } from "react";

import {
  type Site,
  type SiteBody,
  type SiteSpec,
  useSiteLog,
  useSitePreview,
} from "../../api/ingress";
import { useServers } from "../../api/hooks";
import { CodeField } from "../CodeField";
import {
  CopyButton,
  ErrorNote,
  IconButton,
  Mono,
  Select,
  bytes,
  tabTrigger,
  useDebounced,
} from "../ui";
import { type EndpointForm, toBody } from "./endpointForm";
import { Check } from "./EndpointParts";

/** Config file: the generated main configuration (read-only) and the editable custom configuration. */
export function ConfigSection({
  f,
  setSpec,
}: {
  f: EndpointForm;
  setSpec: (p: Partial<SiteSpec>) => void;
}) {
  const [tab, setTab] = useState<"main" | "custom">("main");
  // Debounce the serialised body: a fresh object every render would never settle.
  const json = useDebounced(JSON.stringify(toBody(f)), 500);
  const body = useMemo(() => JSON.parse(json) as SiteBody, [json]);
  const preview = useSitePreview(body);
  const p = preview.data;
  const ce = p?.custom_error ?? null;
  const main = p
    ? (p.config ?? (f.spec.tls !== "off" ? p.https : p.http) ?? "")
    : "";

  return (
    <div className="flex min-h-0 flex-col gap-2">
      <div
        role="tablist"
        aria-label="Config file"
        className="flex items-center gap-1 border-b border-line"
      >
        {(
          [
            ["main", "Main configuration"],
            ["custom", "Custom configuration"],
          ] as const
        ).map(([id, label]) => (
          <button
            key={id}
            type="button"
            role="tab"
            aria-selected={tab === id}
            data-state={tab === id ? "active" : "inactive"}
            className={tabTrigger}
            onClick={() => setTab(id)}
          >
            {label}
          </button>
        ))}
        {preview.isFetching && (
          <Loader2
            className="ml-auto size-3.5 animate-spin text-faint"
            aria-label="Rendering"
          />
        )}
      </div>

      {tab === "main" && (
        <>
          <div className="flex items-center gap-2 text-xs text-muted">
            <span className="min-w-0 flex-1">
              Forge rewrites this file on every apply; put your own directives
              in the custom configuration.
            </span>
            {main && <CopyButton text={main} />}
          </div>
          {preview.error ? (
            <ErrorNote error={preview.error} />
          ) : preview.isLoading ? (
            <p className="text-xs text-faint">Rendering…</p>
          ) : (
            <pre className="log-console max-h-[55vh] min-h-[200px] overflow-auto rounded-md bg-term p-3 font-mono text-[11.5px] leading-relaxed text-term-fg select-text">
              {main || "—"}
            </pre>
          )}
        </>
      )}

      {tab === "custom" && (
        <>
          <div className="space-y-1 text-xs text-muted">
            <p>
              Nginx directives included inside this endpoint's{" "}
              <Mono>server</Mono> block(s), for what the form doesn't cover
              (extra headers, locations, caching rules).
            </p>
            <ul className="list-disc space-y-0.5 pl-4 text-[11px]">
              <li>
                Checked against an allowlist of directives: no file paths, no
                process or code directives. <Mono>auth_basic_user_file</Mono>,{" "}
                <Mono>access_log</Mono> and <Mono>error_log</Mono> may only
                point at this endpoint's own managed files.
              </li>
              <li>
                Tested with <Mono>nginx -t</Mono> on each edge and rolled back
                if the test fails.
              </li>
              <li>
                Saving or applying a non-empty custom file needs the "Run
                commands" permission (with MFA) on every edge. Changes are
                recorded in the audit log.
              </li>
            </ul>
          </div>
          <CodeField
            lang="nginx"
            aria-label="Custom configuration"
            minLines={14}
            maxLines={36}
            value={f.spec.custom}
            onChange={(custom) => setSpec({ custom })}
            invalid={!!ce}
            placeholder={
              'add_header X-Frame-Options SAMEORIGIN;\nlocation = /robots.txt {\n    return 200 "User-agent: *\\nDisallow:\\n";\n}'
            }
          />
          {ce ? (
            <p
              className="rounded-md bg-bad-soft px-2 py-1 text-xs text-bad"
              role="alert"
            >
              Line {ce.line}:{" "}
              {ce.directive ? <Mono>{ce.directive}</Mono> : null} {ce.message}
            </p>
          ) : preview.error ? (
            <ErrorNote error={preview.error} />
          ) : (
            f.spec.custom.trim() &&
            p && (
              <p className="text-xs text-ok">
                Accepted by the directive check (nginx -t runs on apply).
              </p>
            )
          )}
        </>
      )}
    </div>
  );
}

const TAILS = [100, 200, 500, 1000];

/** Logs: the access or error log of this endpoint on one of its edges. */
export function LogsSection({ site }: { site: Site }) {
  const nodes = useServers().data ?? [];
  const edges = site.config.edges;
  const [v, setV] = useState({
    server: edges[0] ?? "",
    log: "access" as "access" | "error",
    tail: 200,
    follow: false,
  });
  const server = edges.includes(v.server) ? v.server : (edges[0] ?? "");
  const q = useSiteLog(site.id, server, v.log, v.follow);
  const host = (id: string) =>
    nodes.find((n) => n.id === id)?.hostname ?? id.slice(0, 8);
  const d = q.data;
  const text = (d?.result?.log ?? "").split("\n");
  const shown = text.slice(Math.max(0, text.length - v.tail)).join("\n");

  if (!edges.length)
    return (
      <p className="text-xs text-muted">
        This endpoint has no edges yet. Apply it first; its logs live on the
        edge nodes.
      </p>
    );
  return (
    <div className="flex min-h-0 flex-col gap-2">
      <div className="flex flex-wrap items-center gap-2">
        <div className="w-44">
          <Select
            aria-label="Edge"
            value={server}
            onChange={(e) => setV({ ...v, server: e.target.value })}
          >
            {edges.map((e) => (
              <option key={e} value={e}>
                {host(e)}
              </option>
            ))}
          </Select>
        </div>
        <div
          className="inline-flex rounded-md border border-line p-0.5"
          role="radiogroup"
          aria-label="Log"
        >
          {(["access", "error"] as const).map((l) => (
            <button
              key={l}
              type="button"
              role="radio"
              aria-checked={v.log === l}
              onClick={() => setV({ ...v, log: l })}
              className={`rounded px-2 py-0.5 text-xs ${v.log === l ? "bg-accent-soft text-accent" : "text-muted hover:text-fg"}`}
            >
              {l === "access" ? "Access log" : "Error log"}
            </button>
          ))}
        </div>
        <div className="w-28">
          <Select
            aria-label="Lines"
            value={v.tail}
            onChange={(e) => setV({ ...v, tail: Number(e.target.value) })}
          >
            {TAILS.map((n) => (
              <option key={n} value={n}>
                last {n}
              </option>
            ))}
          </Select>
        </div>
        <Check checked={v.follow} onChange={(follow) => setV({ ...v, follow })}>
          Follow
        </Check>
        <span className="ml-auto flex items-center gap-2 font-mono text-[11px] text-faint tabular-nums">
          {d?.result?.path && (
            <span className="max-w-[260px] truncate" title={d.result.path}>
              {d.result.path}
            </span>
          )}
          {typeof d?.result?.size === "number" ? bytes(d.result.size) : "—"}
          <IconButton label="Refresh" onClick={() => void q.refetch()}>
            <RefreshCw
              className={`size-4 ${q.isFetching ? "animate-spin" : ""}`}
            />
          </IconButton>
          {shown && <CopyButton text={shown} />}
        </span>
      </div>
      {q.error ? (
        <ErrorNote error={q.error} />
      ) : q.isLoading ? (
        <p className="text-xs text-faint">Reading the log on {host(server)}…</p>
      ) : d?.status === "failed" ? (
        <ErrorNote error={d.error ?? "Reading the log failed"} />
      ) : d?.status === "running" ? (
        <p className="text-xs text-muted">
          The edge is still reading the log; it refreshes shortly.
        </p>
      ) : d?.result?.missing ? (
        <p className="text-xs text-muted">
          No {v.log} log on {host(server)} yet: the endpoint hasn't been applied
          there or has had no requests.
        </p>
      ) : (
        <pre className="log-console max-h-[55vh] min-h-[200px] overflow-auto rounded-md bg-term p-3 font-mono text-[11px] leading-relaxed whitespace-pre-wrap text-term-fg select-text">
          {shown.trim() ? shown : "The log is empty."}
        </pre>
      )}
    </div>
  );
}
