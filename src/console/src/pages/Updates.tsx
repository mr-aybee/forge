import { Link, useNavigate, useSearch } from "@tanstack/react-router";
import { ArrowRight, BellRing, CheckCircle2, ExternalLink, RefreshCw, ShieldCheck, Zap } from "lucide-react";
import { useMemo, useState } from "react";

import { useServices, useApplyUpdate, useCheckUpdates, usePackageUpdates, useServers, useUpdatePolicy } from "../api/hooks";
import type { UpdateKind, UpdateRow } from "../api/types";
import { errorText, useToast } from "../app/toast";
import { Badge, Button, Empty, ErrorNote, Modal, Mono, PageHeader, Panel, Select, Skeleton, Table, Td, ago } from "../components/ui";
import { FilterBar } from "../components/filters/FilterBar";
import type { UpdatesSearch } from "../app/router";
import { csv, one, toCsv } from "../lib/search";

const KIND: Record<UpdateKind, { label: string; tone: "ok" | "accent" | "warn" | "bad"; hint: string }> = {
  revision: { label: "security / packaging", tone: "ok", hint: "Same upstream version, new package revision (often a security backport)." },
  patch: { label: "patch", tone: "ok", hint: "Bug and security fixes; no behaviour changes expected." },
  minor: { label: "minor", tone: "accent", hint: "New features; read the release notes first." },
  major: { label: "major", tone: "bad", hint: "Breaking changes are likely; plan the upgrade." },
};

/** `true`/`false` or a date: is the line past end of life, or how many days are left. */
function eolInfo(eol: string | null): { past: boolean; days?: number } | null {
  if (!eol) return null;
  if (eol === "true") return { past: true };
  if (eol === "false") return null;
  const t = Date.parse(eol);
  if (Number.isNaN(t)) return null;
  const days = Math.ceil((t - Date.now()) / 86_400_000);
  return { past: days <= 0, days };
}

function Lifecycle({ r }: { r: UpdateRow }) {
  const u = r.upstream;
  if (!u) return <span className="text-xs text-faint">—</span>;
  const eol = eolInfo(u.eol);
  return (
    <div className="flex flex-wrap items-center gap-1">
      <Badge tone="muted">line {u.line}</Badge>
      {u.lts && <Badge tone="ok">LTS</Badge>}
      {!u.newest_line && <Badge tone="accent">latest</Badge>}
      {eol?.past ? <Badge tone="bad">EOL</Badge> : eol?.days !== undefined && eol.days < 180 ? <Badge tone="warn">EOL in {eol.days} d</Badge> : null}
      {u.newest_line && <span className="text-[11px] text-faint">newest line {u.newest_line}</span>}
    </div>
  );
}

/** Installed packages from package repositories, and the updates their nodes offer. */
export function Updates() {
  const rows = usePackageUpdates();
  const nodes = useServers().data ?? [];
  const apps = useServices().data ?? [];
  const check = useCheckUpdates();
  const apply = useApplyUpdate();
  const policy = useUpdatePolicy();
  const toast = useToast();
  const search = useSearch({ from: "/apps-updates" });
  const navigate = useNavigate();
  const patch = (p: Partial<UpdatesSearch>) => void navigate({ from: "/apps-updates", to: "/apps-updates", search: (s) => ({ ...s, ...p }), replace: true });
  const q = search.q ?? "";
  const show = search.show ?? "updates";
  const kinds = csv(search.kind);
  const [confirm, setConfirm] = useState<UpdateRow>();

  const host = (id: string) => nodes.find((n) => n.id === id)?.hostname ?? id.slice(0, 8);
  const all = rows.data ?? [];
  const pending = all.filter((r) => r.kind);
  const base = all.filter(
    (r) => (!search.node || r.server_id === search.node) && (!kinds.length || (!!r.kind && kinds.includes(r.kind))) && `${r.name} ${r.package} ${host(r.server_id)}`.toLowerCase().includes(q.toLowerCase()),
  );
  const list = show === "updates" ? base.filter((r) => r.kind) : base;
  const nodeIds = [...new Set(all.map((r) => r.server_id))].sort((a, b) => host(a).localeCompare(host(b)));
  const lastCheck = Math.max(0, ...all.map((r) => r.checked_at ?? 0));

  /** Services on the same node that use this package (dependency or its env file). */
  const dependents = useMemo(
    () => (r: UpdateRow) =>
      apps.filter(
        (a) =>
          a.instances.some((i) => i.server_id === r.server_id) &&
          ((a.spec.dependencies ?? []).some((d) => d.package === r.package) || a.spec.env_files.some((f) => f.includes(`/pkgs/${r.package}.env`))),
      ),
    [apps],
  );

  const runCheck = () =>
    check.mutate(undefined, {
      onSuccess: (r) => toast(r.checks ? `Checking ${r.checks} installation${r.checks === 1 ? "" : "s"} for updates` : "Nothing to check"),
      onError: (e) => toast(errorText(e), "error"),
    });

  const doUpdate = (r: UpdateRow) =>
    apply.mutate(
      { id: r.installation_id, build: r.candidate },
      {
        onSuccess: () => {
          toast(`Updating ${r.name} on ${host(r.server_id)}`);
          setConfirm(undefined);
        },
        onError: (e) => toast(errorText(e), "error"),
      },
    );

  return (
    <>
      <PageHeader
        eyebrow="Packages"
        title="Package updates"
        description="Updates for packages installed on your nodes, from each node's own package repositories (apt, dnf). Vendor release lines show which versions are LTS, latest or near end of life. Forge and its agents update under Administration → Forge updates."
        actions={
          <Button onClick={runCheck} disabled={check.isPending}>
            <RefreshCw className={`size-4 ${check.isPending ? "animate-spin" : ""}`} /> Check now
          </Button>
        }
      />
      <div className="mb-4 grid gap-3 sm:grid-cols-3">
        <Summary icon={<BellRing className="size-4" />} label="Updates available" value={pending.length} tone={pending.length ? "warn" : "ok"} />
        <Summary icon={<ShieldCheck className="size-4" />} label="Patch and security" value={pending.filter((r) => r.kind === "patch" || r.kind === "revision").length} />
        <Summary icon={<RefreshCw className="size-4" />} label="Last checked" value={lastCheck ? ago(lastCheck) : "never"} />
      </div>
      <FilterBar
        status={{
          label: "Show",
          value: show,
          onChange: (v) => patch({ show: v === "all" ? "all" : undefined }),
          options: [
            { id: "updates", label: "With updates", count: base.filter((r) => r.kind).length },
            { id: "all", label: "All tracked", count: base.length },
          ],
        }}
        search={{ value: q, onChange: (v) => patch({ q: v || undefined }), placeholder: "Search packages or nodes" }}
        filters={[
          {
            id: "kind",
            label: "Kind",
            multi: true,
            value: kinds,
            onChange: (v) => patch({ kind: toCsv(v) }),
            options: (Object.keys(KIND) as UpdateKind[]).map((k) => ({ value: k, label: KIND[k].label, count: pending.filter((r) => r.kind === k).length })),
          },
          { id: "node", label: "Node", value: one(search.node), onChange: (v) => patch({ node: v[0] }), options: nodeIds.map((id) => ({ value: id, label: host(id) })) },
        ]}
        onClear={() => patch({ q: undefined, kind: undefined, node: undefined })}
      />
      <Panel title={show === "updates" ? `${list.length} update${list.length === 1 ? "" : "s"}` : `${list.length} package${list.length === 1 ? "" : "s"}`}>
        <ErrorNote error={rows.error} />
        {rows.isLoading ? (
          <Skeleton />
        ) : list.length === 0 ? (
          <Empty icon={<CheckCircle2 className="size-5" />} title={all.length ? (show === "updates" ? "Everything is up to date" : "Nothing matches") : "No tracked packages"}>
            {all.length
              ? lastCheck
                ? `Last checked ${ago(lastCheck)}. Forge checks every six hours.`
                : "Run a check to ask your nodes what their repositories offer."
              : "Packages installed from the App Store through apt or dnf appear here."}
          </Empty>
        ) : (
          <Table head={["Package", "Node", "Installed", "", "Available", "Release line", "Policy", ""]}>
            {list.map((r) => {
              const busy = r.status.endsWith("ing");
              const k = r.kind ? KIND[r.kind] : null;
              return (
                <tr key={r.installation_id} className="hover:bg-panel-2/60">
                  <Td>
                    <div className="font-medium">{r.name}</div>
                    <div className="text-[11px] text-faint">
                      {r.manager || "—"} · <Mono>{r.os_package || r.package}</Mono>
                    </div>
                  </Td>
                  <Td>
                    <Link to="/nodes/$nodeId" params={{ nodeId: r.server_id }} className="hover:text-accent">
                      {host(r.server_id)}
                    </Link>
                  </Td>
                  <Td>
                    <Mono>{r.installed || "unknown"}</Mono>
                    {r.running && <div className="text-[11px] text-faint">running {r.running}</div>}
                  </Td>
                  <Td>{r.kind && <ArrowRight className="size-3.5 text-faint" />}</Td>
                  <Td>
                    {r.kind ? (
                      <div className="space-y-0.5">
                        <Mono>{r.candidate}</Mono>
                        <div>
                          <span title={k?.hint}>
                            <Badge tone={k?.tone}>{k?.label}</Badge>
                          </span>
                        </div>
                      </div>
                    ) : (
                      <span className="text-xs text-ok">up to date</span>
                    )}
                    {r.upstream?.ahead_of_node && (
                      <div className="mt-0.5 text-[11px] text-warn" title="The vendor has released a newer build that this node's repositories do not offer yet.">
                        {r.upstream.latest} released upstream · not on this node yet
                      </div>
                    )}
                  </Td>
                  <Td>
                    <Lifecycle r={r} />
                  </Td>
                  <Td>
                    <Select
                      className="h-7 w-auto py-0 text-xs"
                      value={r.update_policy}
                      aria-label="Update policy"
                      onChange={(e) =>
                        policy.mutate(
                          { id: r.installation_id, update_policy: e.target.value as "notify" | "auto_patch" },
                          { onSuccess: () => toast("Update policy saved"), onError: (er) => toast(errorText(er), "error") },
                        )
                      }
                    >
                      <option value="notify">Notify me</option>
                      <option value="auto_patch">Auto-apply patches</option>
                    </Select>
                  </Td>
                  <Td className="text-right whitespace-nowrap">
                    {r.release_notes && (
                      <a href={r.release_notes} target="_blank" rel="noreferrer" className="mr-2 inline-flex items-center gap-1 text-xs text-muted hover:text-accent">
                        Notes <ExternalLink className="size-3" />
                      </a>
                    )}
                    {r.kind && (
                      <Button variant="primary" className="h-7 px-2.5 text-xs" disabled={busy} onClick={() => setConfirm(r)}>
                        <Zap className="size-3.5" /> {busy ? r.status : "Update"}
                      </Button>
                    )}
                  </Td>
                </tr>
              );
            })}
          </Table>
        )}
      </Panel>

      {confirm && (
        <Modal open onOpenChange={(o) => !o && setConfirm(undefined)} title={`Update ${confirm.name} on ${host(confirm.server_id)}?`}>
          <div className="space-y-3 text-sm">
            <div className="flex items-center gap-2 rounded-md border border-line bg-panel-2/50 px-3 py-2 font-mono text-xs">
              {confirm.installed} <ArrowRight className="size-3.5 text-faint" /> {confirm.candidate}
              {confirm.kind && (
                <span className="ml-auto">
                  <Badge tone={KIND[confirm.kind].tone}>{KIND[confirm.kind].label}</Badge>
                </span>
              )}
            </div>
            <p className="text-warn">
              The service restarts to load the new build. Expect a short outage of {confirm.name} on {host(confirm.server_id)}.
            </p>
            {(() => {
              const deps = dependents(confirm);
              return deps.length ? (
                <div>
                  <div className="mb-1 text-xs font-medium text-muted">Services on this node that use it</div>
                  <ul className="space-y-0.5 text-xs">
                    {deps.map((a) => (
                      <li key={a.id}>
                        <Link to="/services/$serviceId" params={{ serviceId: a.id }} className="hover:text-accent">
                          {a.name}
                        </Link>
                      </li>
                    ))}
                  </ul>
                </div>
              ) : (
                <p className="text-xs text-muted">No Forge services on this node depend on it.</p>
              );
            })()}
            {confirm.kind && <p className="text-xs text-muted">{KIND[confirm.kind].hint}</p>}
            <p className="text-xs text-faint">
              Forge installs exactly {confirm.candidate} with {confirm.manager}, then checks that the service reports the new version.
            </p>
          </div>
          <div className="mt-5 flex justify-end gap-2">
            <Button onClick={() => setConfirm(undefined)}>Cancel</Button>
            <Button variant="primary" disabled={apply.isPending} onClick={() => doUpdate(confirm)}>
              <Zap className="size-4" /> Update now
            </Button>
          </div>
        </Modal>
      )}
    </>
  );
}

function Summary({ icon, label, value, tone }: { icon: React.ReactNode; label: string; value: React.ReactNode; tone?: "ok" | "warn" }) {
  return (
    <div className="flex items-center gap-3 rounded-lg border border-line bg-panel px-3 py-2.5">
      <span className={`grid size-8 place-items-center rounded-md ${tone === "warn" ? "bg-warn-soft text-warn" : tone === "ok" ? "bg-ok-soft text-ok" : "bg-accent-soft text-accent"}`}>{icon}</span>
      <div>
        <div className="text-[11px] text-faint">{label}</div>
        <div className="text-sm font-semibold tabular-nums">{value}</div>
      </div>
    </div>
  );
}
