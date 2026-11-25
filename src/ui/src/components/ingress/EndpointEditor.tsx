import {
  ArrowRightLeft,
  FileCode2,
  Gauge,
  Globe,
  Loader2,
  Lock,
  Plus,
  Rocket,
  Route,
  ScrollText,
  Server,
  ShieldCheck,
  Trash2,
  X,
} from "lucide-react";
import { type ReactNode, useState } from "react";

import { useServers, useInstallations } from "../../api/hooks";
import {
  type DnsCfg,
  type Site,
  type SiteSpec,
  useClusterProfile,
  useSiteRun,
  useSites,
  useUpdateSite,
} from "../../api/ingress";
import { errorText, useToast } from "../../app/toast";
import {
  Badge,
  Button,
  Confirm,
  ErrorNote,
  Field,
  IconButton,
  Input,
  Modal,
  Select,
  Skeleton,
  Status,
  TextArea,
  ago,
  time,
} from "../ui";
import { ConfigSection, LogsSection } from "./EditorConfig";
import { RoutingSection } from "./EditorRouting";
import { SslSection } from "./EditorSsl";
import {
  type EndpointForm,
  type FormUser,
  blankDns,
  formFromSite,
  formProblems,
  lines,
  newKey,
  toBody,
  usePlacementOptions,
} from "./endpointForm";
import { Check, PlacementPicker, SectionHead } from "./EndpointParts";

type SectionId =
  | "domains"
  | "routing"
  | "performance"
  | "access"
  | "ssl"
  | "redirects"
  | "config"
  | "logs"
  | "placement";
const SECTIONS: { id: SectionId; label: string; icon: typeof Globe }[] = [
  { id: "domains", label: "Domains", icon: Globe },
  { id: "routing", label: "Routing", icon: Route },
  { id: "performance", label: "Performance", icon: Gauge },
  { id: "access", label: "Access", icon: ShieldCheck },
  { id: "ssl", label: "SSL", icon: Lock },
  { id: "redirects", label: "Redirects", icon: ArrowRightLeft },
  { id: "config", label: "Config file", icon: FileCode2 },
  { id: "logs", label: "Logs", icon: ScrollText },
  { id: "placement", label: "Placement", icon: Server },
];

/** The full endpoint editor for a saved endpoint, looked up by id so it follows the list. */
export function EndpointEditor({
  siteId,
  onClose,
}: {
  siteId: string;
  onClose: () => void;
}) {
  const sites = useSites();
  const site = sites.data?.find((s) => s.id === siteId);
  if (!site) {
    return (
      <Modal open onOpenChange={(o) => !o && onClose()} title="Endpoint">
        {sites.error ? (
          <ErrorNote error={sites.error} />
        ) : sites.isLoading ? (
          <Skeleton rows={3} />
        ) : (
          <p className="text-xs text-muted">This endpoint no longer exists.</p>
        )}
      </Modal>
    );
  }
  return <Editor key={site.id} site={site} onClose={onClose} />;
}

const snapshot = (f: EndpointForm) => JSON.stringify(toBody(f));

function Editor({ site, onClose }: { site: Site; onClose: () => void }) {
  const toast = useToast();
  const [state, setState] = useState(() => {
    const f = formFromSite(site);
    return { f, baseline: snapshot(f) };
  });
  const [section, setSection] = useState<SectionId>("domains");
  const [discard, setDiscard] = useState(false);
  const f = state.f;
  const set = (p: Partial<EndpointForm>) =>
    setState((s) => ({ ...s, f: { ...s.f, ...p } }));
  const setSpec = (p: Partial<SiteSpec>) =>
    setState((s) => ({ ...s, f: { ...s.f, spec: { ...s.f.spec, ...p } } }));
  const dirty = snapshot(f) !== state.baseline;
  const problems = formProblems(f);
  const update = useUpdateSite();
  const run = useSiteRun();
  const nodes = useServers().data ?? [];
  const host = (id: string) =>
    nodes.find((n) => n.id === id)?.hostname ?? id.slice(0, 8);

  const save = () =>
    update.mutate(
      { id: site.id, body: toBody(f) },
      {
        onSuccess: () => {
          // Passwords are stored now: the users keep them unless changed again.
          setState((s) => {
            const next = {
              ...s.f,
              users: s.f.users.map((u) => ({
                ...u,
                saved: true,
                password: "",
              })),
            };
            return { f: next, baseline: snapshot(next) };
          });
          toast("Saved. Press Apply to push it to the edges.");
        },
        onError: (e) => toast(errorText(e), "error"),
      },
    );
  const apply = () =>
    run.mutate(
      { id: site.id, action: "apply" },
      {
        onSuccess: (r) => {
          const failed = (r.edges ?? []).filter(
            (e) => e.result?.status !== "succeeded",
          );
          if (failed.length)
            toast(
              `Apply failed on ${failed.map((x) => host(x.server_id)).join(", ")}: ${failed[0]?.result?.error ?? ""}`,
              "error",
            );
          else
            toast(
              `Applied on ${r.edges?.length ?? 0} edge${r.edges?.length === 1 ? "" : "s"}`,
            );
        },
        onError: (e) => toast(errorText(e), "error"),
      },
    );

  const body: Record<SectionId, ReactNode> = {
    domains: <DomainsSection f={f} setSpec={setSpec} />,
    routing: <RoutingSection f={f} set={set} />,
    performance: (
      <PerformanceSection f={f} setSpec={setSpec} site={site} dirty={dirty} />
    ),
    access: <AccessSection f={f} set={set} setSpec={setSpec} />,
    ssl: <SslSection f={f} setSpec={setSpec} site={site} dirty={dirty} />,
    redirects: <RedirectsSection f={f} set={set} />,
    config: <ConfigSection f={f} setSpec={setSpec} />,
    logs: <LogsSection site={site} />,
    placement: <PlacementSection f={f} set={set} site={site} />,
  };

  return (
    <Modal
      open
      onOpenChange={(o) => !o && (dirty ? setDiscard(true) : onClose())}
      title={`Endpoint · ${site.name}`}
      description={f.spec.domains[0] ?? undefined}
      width={1080}
      fill
    >
      <div className="flex h-[70vh] min-h-0 flex-col sm:flex-row">
        <nav
          aria-label="Endpoint sections"
          className="flex shrink-0 gap-0.5 overflow-x-auto border-b border-line pb-1 sm:w-44 sm:flex-col sm:overflow-visible sm:border-r sm:border-b-0 sm:pr-2 sm:pb-0"
        >
          {SECTIONS.map(({ id, label, icon: Icon }) => (
            <button
              key={id}
              type="button"
              aria-current={section === id ? "page" : undefined}
              onClick={() => setSection(id)}
              className={`flex shrink-0 items-center gap-2 rounded-md px-2 py-1.5 text-left text-[13px] focus-visible:outline-2 focus-visible:outline-accent ${section === id ? "bg-accent-soft text-accent" : "text-muted hover:bg-panel-2 hover:text-fg"}`}
            >
              <Icon className="size-3.5 shrink-0" /> {label}
            </button>
          ))}
        </nav>
        <div className="min-h-0 min-w-0 flex-1 overflow-y-auto py-2 sm:py-0 sm:pl-4">
          {body[section]}
        </div>
      </div>
      <div className="mt-3 flex shrink-0 flex-wrap items-center gap-2 border-t border-line pt-3">
        <span className="min-w-0 flex-1 truncate text-xs" title={problems[0]}>
          {problems.length ? (
            <span className="text-warn">{problems[0]}</span>
          ) : dirty ? (
            <span className="text-muted">Unsaved changes</span>
          ) : (
            <span className="text-faint">
              Saved {ago(site.updated_at)}. Saving never applies.
            </span>
          )}
        </span>
        <Button
          type="button"
          variant="ghost"
          onClick={() => (dirty ? setDiscard(true) : onClose())}
        >
          Close
        </Button>
        <Button
          type="button"
          disabled={run.isPending || dirty}
          title={
            dirty
              ? "Save first: Apply pushes the saved endpoint"
              : "Push the saved config to every edge (tested with nginx -t first)"
          }
          onClick={apply}
        >
          {run.isPending ? (
            <Loader2 className="size-4 animate-spin" />
          ) : (
            <Rocket className="size-4" />
          )}{" "}
          Apply
        </Button>
        <Button
          type="button"
          variant="primary"
          disabled={update.isPending || !dirty || problems.length > 0}
          title={problems[0]}
          onClick={save}
        >
          {update.isPending && <Loader2 className="size-4 animate-spin" />} Save
        </Button>
      </div>
      {discard && (
        <Confirm
          open
          onOpenChange={(o) => !o && setDiscard(false)}
          title="Discard unsaved changes?"
          body="Your edits to this endpoint are lost. Nothing on the edges changes."
          confirmLabel="Discard"
          onConfirm={onClose}
        />
      )}
    </Modal>
  );
}

// ------------------------------------------------------------------ Domains

function DomainsSection({
  f,
  setSpec,
}: {
  f: EndpointForm;
  setSpec: (p: Partial<SiteSpec>) => void;
}) {
  const [v, setV] = useState({ adding: "", selected: [] as string[] });
  const domains = f.spec.domains;
  const acme = f.spec.tls === "acme";
  const add = () => {
    const next = [
      ...domains,
      ...lines(v.adding)
        .map((d) => d.toLowerCase())
        .filter((d) => !domains.includes(d)),
    ];
    setSpec({ domains: [...new Set(next)] });
    setV({ ...v, adding: "" });
  };
  return (
    <div className="space-y-4">
      <section>
        <SectionHead
          title={`Domains (${domains.length})`}
          hint="The first is the primary. Point each one's DNS at the edges."
          actions={
            <Button
              type="button"
              variant="danger"
              disabled={!v.selected.length}
              onClick={() => (
                setSpec({
                  domains: domains.filter((d) => !v.selected.includes(d)),
                }),
                setV({ ...v, selected: [] })
              )}
            >
              <Trash2 className="size-4" /> Delete selected
            </Button>
          }
        />
        {domains.length === 0 ? (
          <p className="text-xs text-warn">
            No domains: add at least one below.
          </p>
        ) : (
          <ul className="divide-y divide-line rounded-md border border-line">
            {domains.map((d, i) => (
              <li key={d} className="flex min-w-0 items-center gap-2 px-2 py-1">
                <input
                  type="checkbox"
                  aria-label={`Select ${d}`}
                  checked={v.selected.includes(d)}
                  onChange={(e) =>
                    setV({
                      ...v,
                      selected: e.target.checked
                        ? [...v.selected, d]
                        : v.selected.filter((x) => x !== d),
                    })
                  }
                />
                <span
                  className="min-w-0 flex-1 truncate font-mono text-xs select-text"
                  title={d}
                >
                  {d}
                </span>
                {i === 0 ? (
                  <Badge tone="accent">primary</Badge>
                ) : (
                  <button
                    type="button"
                    className="text-[11px] text-muted hover:text-accent"
                    onClick={() =>
                      setSpec({
                        domains: [d, ...domains.filter((x) => x !== d)],
                      })
                    }
                  >
                    make primary
                  </button>
                )}
                {d.includes("*") && (
                  <Badge tone={acme ? "bad" : "muted"}>wildcard</Badge>
                )}
              </li>
            ))}
          </ul>
        )}
        <div className="mt-2 flex items-end gap-2">
          <div className="min-w-0 flex-1">
            <Field
              label="Add domains"
              hint={
                acme
                  ? "One per line. Wildcards (*.example.com) need an uploaded certificate, not Let's Encrypt."
                  : "One per line. Wildcards (*.example.com) are allowed with an uploaded certificate."
              }
            >
              <TextArea
                rows={2}
                className="font-mono text-xs"
                value={v.adding}
                onChange={(e) => setV({ ...v, adding: e.target.value })}
                placeholder="www.example.com"
              />
            </Field>
          </div>
          <Button type="button" disabled={!v.adding.trim()} onClick={add}>
            <Plus className="size-4" /> Add
          </Button>
        </div>
      </section>
      <section>
        <SectionHead title="Ports" hint="Where Nginx listens on the edges." />
        <div className="grid gap-3 sm:grid-cols-2">
          <Field
            label="HTTP port"
            hint={
              acme && (f.spec.http_port ?? 80) !== 80
                ? "Let's Encrypt validates on port 80; keep 80 while it is in use."
                : "Default 80."
            }
          >
            <Input
              type="number"
              min={1}
              max={65535}
              value={f.spec.http_port ?? 80}
              onChange={(e) =>
                setSpec({ http_port: Number(e.target.value) || 80 })
              }
            />
          </Field>
          <Field label="HTTPS port" hint="Default 443.">
            <Input
              type="number"
              min={1}
              max={65535}
              value={f.spec.https_port ?? 443}
              onChange={(e) =>
                setSpec({ https_port: Number(e.target.value) || 443 })
              }
            />
          </Field>
        </div>
      </section>
    </div>
  );
}

// ------------------------------------------------------------------ Performance

function PerformanceSection({
  f,
  setSpec,
  site,
  dirty,
}: {
  f: EndpointForm;
  setSpec: (p: Partial<SiteSpec>) => void;
  site: Site;
  dirty: boolean;
}) {
  const toast = useToast();
  const run = useSiteRun();
  const c = f.spec.cache;
  const g = f.spec.gzip;
  const savedCache = !!site.config.spec.cache;
  return (
    <div className="space-y-5">
      <section>
        <SectionHead
          title="Cache"
          hint="Nginx keeps copies of upstream responses on each edge."
          actions={
            <Button
              type="button"
              disabled={!savedCache || run.isPending}
              title={
                savedCache
                  ? "Empty this endpoint's cache on every edge"
                  : "Save with the cache on (and Apply) first"
              }
              onClick={() =>
                run.mutate(
                  { id: site.id, action: "cache/clear" },
                  {
                    onSuccess: (r) => {
                      const bad = (r.edges ?? []).filter(
                        (e) => e.result?.status !== "succeeded",
                      );
                      toast(
                        bad.length
                          ? `Clearing failed on ${bad.length} edge${bad.length === 1 ? "" : "s"}: ${bad[0]?.result?.error ?? ""}`
                          : "Cache cleared on every edge",
                        bad.length ? "error" : "ok",
                      );
                    },
                    onError: (e) => toast(errorText(e), "error"),
                  },
                )
              }
            >
              {run.isPending && <Loader2 className="size-4 animate-spin" />}{" "}
              Clear cache
            </Button>
          }
        />
        <Check
          checked={!!c}
          onChange={(on) =>
            setSpec({
              cache: on
                ? {
                    key: "$scheme$host$request_uri",
                    extensions: [],
                    ttl_minutes: 60,
                    ignore_headers: false,
                    max_size_mb: 256,
                  }
                : undefined,
            })
          }
        >
          Cache responses
        </Check>
        {c && (
          <div className="mt-2 grid gap-3 sm:grid-cols-2">
            <Field label="Cache key">
              <Input
                className="font-mono text-xs"
                value={c.key}
                onChange={(e) =>
                  setSpec({ cache: { ...c, key: e.target.value } })
                }
              />
            </Field>
            <Field
              label="File extensions"
              hint="Comma separated; empty caches every cacheable response."
            >
              <Input
                className="font-mono text-xs"
                value={c.extensions.join(", ")}
                placeholder="css, js, png, jpg, svg, woff2"
                onChange={(e) =>
                  setSpec({
                    cache: { ...c, extensions: lines(e.target.value) },
                  })
                }
              />
            </Field>
            <Field label="Keep for (minutes)">
              <Input
                type="number"
                min={1}
                value={c.ttl_minutes}
                onChange={(e) =>
                  setSpec({
                    cache: { ...c, ttl_minutes: Number(e.target.value) },
                  })
                }
              />
            </Field>
            <Field label="Max size (MiB per edge)">
              <Input
                type="number"
                min={1}
                value={c.max_size_mb}
                onChange={(e) =>
                  setSpec({
                    cache: { ...c, max_size_mb: Number(e.target.value) },
                  })
                }
              />
            </Field>
            <Check
              checked={c.ignore_headers}
              onChange={(ignore_headers) =>
                setSpec({ cache: { ...c, ignore_headers } })
              }
              hint="Cache even when the upstream says not to (Cache-Control, Expires, Set-Cookie)."
            >
              Ignore upstream cache headers
            </Check>
          </div>
        )}
        {dirty && savedCache !== !!c && (
          <p className="mt-1 text-[11px] text-faint">
            Takes effect after Save and Apply.
          </p>
        )}
      </section>
      <section>
        <SectionHead
          title="Compression"
          hint="gzip text responses on the edge."
        />
        <Check
          checked={!!g}
          onChange={(on) =>
            setSpec({
              gzip: on
                ? {
                    level: 5,
                    types: [
                      "text/css",
                      "application/javascript",
                      "application/json",
                      "image/svg+xml",
                    ],
                  }
                : undefined,
            })
          }
        >
          gzip
        </Check>
        {g && (
          <div className="mt-2 grid gap-3 sm:grid-cols-[120px_1fr]">
            <Field label="Level (1-9)">
              <Input
                type="number"
                min={1}
                max={9}
                value={g.level}
                onChange={(e) =>
                  setSpec({
                    gzip: {
                      ...g,
                      level: Math.min(
                        9,
                        Math.max(1, Number(e.target.value) || 1),
                      ),
                    },
                  })
                }
              />
            </Field>
            <Field label="MIME types" hint="text/html is always compressed.">
              <Input
                className="font-mono text-xs"
                value={g.types.join(", ")}
                onChange={(e) =>
                  setSpec({ gzip: { ...g, types: lines(e.target.value) } })
                }
              />
            </Field>
          </div>
        )}
      </section>
    </div>
  );
}

// ------------------------------------------------------------------ Access

function AccessSection({
  f,
  set,
  setSpec,
}: {
  f: EndpointForm;
  set: (p: Partial<EndpointForm>) => void;
  setSpec: (p: Partial<SiteSpec>) => void;
}) {
  const a = f.spec.access ?? { deny: [], allow: [] };
  const setA = (p: Partial<typeof a>) => setSpec({ access: { ...a, ...p } });
  const ba = a.basic_auth;
  const hl = a.hotlink;
  const setUser = (k: string, p: Partial<FormUser>) =>
    set({ users: f.users.map((u) => (u._k === k ? { ...u, ...p } : u)) });
  return (
    <div className="space-y-5">
      <section>
        <SectionHead
          title="IP access"
          hint="IPs or CIDRs, one per line. Deny is checked first; with an allow list, everyone else is refused."
        />
        <div className="grid gap-3 sm:grid-cols-2">
          <Field label="Deny">
            <TextArea
              rows={3}
              className="font-mono text-xs"
              value={a.deny.join("\n")}
              onChange={(e) => setA({ deny: lines(e.target.value) })}
              placeholder="192.0.2.10"
            />
          </Field>
          <Field label="Allow only">
            <TextArea
              rows={3}
              className="font-mono text-xs"
              value={a.allow.join("\n")}
              onChange={(e) => setA({ allow: lines(e.target.value) })}
              placeholder="198.51.100.0/24"
            />
          </Field>
        </div>
      </section>
      <section>
        <SectionHead
          title="Basic authentication"
          hint="Forge stores password hashes only; it can't show existing passwords."
          actions={
            ba && (
              <Button
                type="button"
                onClick={() =>
                  set({
                    users: [
                      ...f.users,
                      { _k: newKey(), user: "", password: "", saved: false },
                    ],
                  })
                }
              >
                <Plus className="size-4" /> Add user
              </Button>
            )
          }
        />
        <Check
          checked={!!ba}
          onChange={(on) =>
            setA({
              basic_auth: on ? { realm: "Restricted", users: [] } : undefined,
            })
          }
        >
          Require a user name and password
        </Check>
        {ba && (
          <div className="mt-2 space-y-2">
            <Field label="Realm">
              <Input
                value={ba.realm}
                onChange={(e) =>
                  setA({ basic_auth: { ...ba, realm: e.target.value } })
                }
              />
            </Field>
            {f.users.length === 0 && (
              <p className="text-xs text-warn">
                Add at least one user, or nobody can get in.
              </p>
            )}
            {f.users.map((u) => (
              <div key={u._k} className="flex items-center gap-2">
                <Input
                  aria-label="User name"
                  className="w-44 font-mono text-xs"
                  value={u.user}
                  disabled={u.saved}
                  title={
                    u.saved
                      ? "Saved users can't be renamed; remove and add one"
                      : undefined
                  }
                  onChange={(e) => setUser(u._k, { user: e.target.value })}
                  placeholder="user"
                />
                <Input
                  aria-label={`Password for ${u.user || "new user"}`}
                  type="password"
                  autoComplete="new-password"
                  className="min-w-0 flex-1 text-xs"
                  value={u.password}
                  onChange={(e) => setUser(u._k, { password: e.target.value })}
                  placeholder={u.saved ? "unchanged" : "password (required)"}
                />
                <IconButton
                  label={`Remove ${u.user || "user"}`}
                  onClick={() =>
                    set({ users: f.users.filter((x) => x._k !== u._k) })
                  }
                >
                  <X className="size-4" />
                </IconButton>
              </div>
            ))}
          </div>
        )}
      </section>
      <section>
        <SectionHead title="Limits" />
        <div className="grid gap-3 sm:grid-cols-3">
          <Field label="Rate limit (req/s per IP)" hint="0 = off.">
            <Input
              type="number"
              min={0}
              max={10000}
              value={f.spec.rate_limit}
              onChange={(e) => setSpec({ rate_limit: Number(e.target.value) })}
            />
          </Field>
          <Field label="Burst" hint="Extra requests queued above the rate.">
            <Input
              type="number"
              min={0}
              disabled={!f.spec.rate_limit}
              value={f.spec.rate_burst ?? 0}
              onChange={(e) => setSpec({ rate_burst: Number(e.target.value) })}
            />
          </Field>
          <Field label="Max request body (MB)">
            <Input
              type="number"
              min={1}
              max={10240}
              value={f.spec.max_body_mb}
              onChange={(e) => setSpec({ max_body_mb: Number(e.target.value) })}
            />
          </Field>
        </div>
      </section>
      <section>
        <SectionHead
          title="Hotlink protection"
          hint="Refuse these files when another site embeds them."
        />
        <Check
          checked={!!hl}
          onChange={(on) =>
            setA({
              hotlink: on
                ? {
                    referers: [],
                    extensions: [
                      "jpg",
                      "jpeg",
                      "png",
                      "gif",
                      "webp",
                      "svg",
                      "mp4",
                    ],
                  }
                : undefined,
            })
          }
        >
          Protect files from hotlinking
        </Check>
        {hl && (
          <div className="mt-2 grid gap-3 sm:grid-cols-2">
            <Field label="Extensions">
              <Input
                className="font-mono text-xs"
                value={hl.extensions.join(", ")}
                onChange={(e) =>
                  setA({
                    hotlink: { ...hl, extensions: lines(e.target.value) },
                  })
                }
              />
            </Field>
            <Field
              label="Also allowed referers"
              hint="This endpoint's own domains are always allowed."
            >
              <Input
                className="font-mono text-xs"
                value={hl.referers.join(", ")}
                placeholder="partner.example.com"
                onChange={(e) =>
                  setA({ hotlink: { ...hl, referers: lines(e.target.value) } })
                }
              />
            </Field>
          </div>
        )}
      </section>
    </div>
  );
}

// ------------------------------------------------------------------ Redirects

function RedirectsSection({
  f,
  set,
}: {
  f: EndpointForm;
  set: (p: Partial<EndpointForm>) => void;
}) {
  const setR = (k: string, p: Partial<EndpointForm["redirects"][number]>) =>
    set({
      redirects: f.redirects.map((r) => (r._k === k ? { ...r, ...p } : r)),
    });
  return (
    <section>
      <SectionHead
        title={`Redirects (${f.redirects.length})`}
        hint="From one of this endpoint's domains (whole host) or a /path, to a URL."
        actions={
          <Button
            type="button"
            onClick={() =>
              set({
                redirects: [
                  ...f.redirects,
                  {
                    _k: newKey(),
                    from: "",
                    to: "",
                    code: 301,
                    keep_path: true,
                  },
                ],
              })
            }
          >
            <Plus className="size-4" /> Add redirect
          </Button>
        }
      />
      {f.redirects.length === 0 ? (
        <p className="text-xs text-faint">No redirects.</p>
      ) : (
        <div className="space-y-1.5">
          <datalist id="endpoint-domains">
            {f.spec.domains.map((d) => (
              <option key={d} value={d} />
            ))}
          </datalist>
          {f.redirects.map((r) => (
            <div
              key={r._k}
              className="flex flex-wrap items-center gap-2 sm:flex-nowrap"
            >
              <Input
                aria-label="From"
                list="endpoint-domains"
                className="w-48 font-mono text-xs"
                value={r.from}
                placeholder="old.example.com or /old"
                onChange={(e) => setR(r._k, { from: e.target.value })}
              />
              <span aria-hidden className="text-faint">
                →
              </span>
              <Input
                aria-label="To"
                className="min-w-0 flex-1 font-mono text-xs"
                value={r.to}
                placeholder="https://example.com/new"
                onChange={(e) => setR(r._k, { to: e.target.value })}
              />
              <div className="w-20">
                <Select
                  aria-label="Code"
                  value={r.code}
                  onChange={(e) => setR(r._k, { code: Number(e.target.value) })}
                >
                  {[301, 302, 307, 308].map((c) => (
                    <option key={c} value={c}>
                      {c}
                    </option>
                  ))}
                </Select>
              </div>
              <Check
                checked={r.keep_path}
                onChange={(keep_path) => setR(r._k, { keep_path })}
              >
                keep path
              </Check>
              <IconButton
                label="Remove redirect"
                onClick={() =>
                  set({ redirects: f.redirects.filter((x) => x._k !== r._k) })
                }
              >
                <X className="size-4" />
              </IconButton>
            </div>
          ))}
        </div>
      )}
    </section>
  );
}

// ------------------------------------------------------------------ Placement

function PlacementSection({
  f,
  set,
  site,
}: {
  f: EndpointForm;
  set: (p: Partial<EndpointForm>) => void;
  site: Site;
}) {
  const opts = usePlacementOptions();
  const nodes = useServers().data ?? [];
  const installs = useInstallations().data ?? [];
  const host = (id: string) =>
    nodes.find((n) => n.id === id)?.hostname ?? id.slice(0, 8);
  const clusterId =
    f.placement.kind === "cluster" ? f.placement.cluster_id : undefined;
  const profileDns = useClusterProfile(clusterId).data?.profile?.ingress?.dns;
  const dnsNodes = nodes.filter((n) =>
    installs.some(
      (i) =>
        i.server_id === n.id &&
        i.package === "powerdns" &&
        i.status === "installed",
    ),
  );
  const dns = f.dns;
  const setDns = (p: Partial<DnsCfg>) =>
    set({ dns: { ...(dns ?? blankDns()), ...p } });
  const moved =
    JSON.stringify(f.placement) !== JSON.stringify(site.config.placement);
  return (
    <div className="space-y-5">
      <section>
        <SectionHead title="Served by" />
        {opts.error ? (
          <ErrorNote error={opts.error} />
        ) : (
          <PlacementPicker
            value={f.placement}
            onChange={(placement) => set({ placement })}
            opts={opts}
          />
        )}
        {moved && site.config.edges.length > 0 && (
          <p className="mt-2 text-xs text-warn">
            Moving the endpoint: remove it from its current edges first (row
            menu → Remove from edges), or its old config stays there.
          </p>
        )}
      </section>
      <section>
        <SectionHead
          title="Apply status"
          hint="Per edge, from the last Apply or Remove."
        />
        {site.config.edges.length === 0 ? (
          <p className="text-xs text-muted">Not on any edge yet.</p>
        ) : (
          <ul className="space-y-1">
            {site.config.edges.map((e) => {
              const st = site.edge_status[e];
              return (
                <li key={e} className="flex min-w-0 items-center gap-2 text-xs">
                  <Status
                    value={
                      st?.status === "applied"
                        ? "online"
                        : st?.status === "failed"
                          ? "failed"
                          : "pending"
                    }
                  />
                  <span className="w-36 truncate font-medium" title={host(e)}>
                    {host(e)}
                  </span>
                  <span
                    className="text-faint"
                    title={st?.updated_at ? time(st.updated_at) : undefined}
                  >
                    {st?.updated_at ? ago(st.updated_at) : "never applied"}
                  </span>
                  {st?.error && (
                    <span
                      className="min-w-0 flex-1 truncate text-bad"
                      title={st.error}
                    >
                      {st.error}
                    </span>
                  )}
                </li>
              );
            })}
          </ul>
        )}
      </section>
      <section>
        <SectionHead title="DNS" />
        {profileDns ? (
          <p className="text-xs text-muted">
            Published by the cluster's ingress profile (
            {host(profileDns.server_id)}, TTL {profileDns.ttl}s
            {profileDns.auto_failover
              ? ", records move off failed edges automatically"
              : ""}
            ). Change it on the Profile tab.
          </p>
        ) : (
          <>
            <Check
              checked={!!dns}
              onChange={(on) => set({ dns: on ? (dns ?? blankDns()) : null })}
            >
              Publish DNS records with PowerDNS when applying
            </Check>
            {dns && (
              <div className="mt-2 grid gap-3 sm:grid-cols-2">
                <Field
                  label="DNS node (PowerDNS installed)"
                  hint={
                    dnsNodes.length
                      ? undefined
                      : "No node has PowerDNS; install it from the App Store."
                  }
                >
                  <Select
                    value={dns.server_id}
                    onChange={(e) => setDns({ server_id: e.target.value })}
                  >
                    <option value="">Choose…</option>
                    {dnsNodes.map((n) => (
                      <option key={n.id} value={n.id}>
                        {n.hostname}
                      </option>
                    ))}
                  </Select>
                </Field>
                <Field label="Zone" hint="With the trailing dot.">
                  <Input
                    className="font-mono text-xs"
                    value={dns.zone}
                    onChange={(e) => setDns({ zone: e.target.value })}
                    placeholder="example.com."
                  />
                </Field>
                <Field
                  label="Edge addresses"
                  hint="Public IPs of the edges, active first."
                >
                  <Input
                    className="font-mono text-xs"
                    value={dns.addresses.join(", ")}
                    onChange={(e) =>
                      setDns({ addresses: lines(e.target.value) })
                    }
                    placeholder="192.0.2.10, 192.0.2.11"
                  />
                </Field>
                <div className="grid grid-cols-[1fr_90px] gap-2">
                  <Field label="Mode">
                    <Select
                      value={dns.mode}
                      onChange={(e) =>
                        setDns({ mode: e.target.value as DnsCfg["mode"] })
                      }
                    >
                      <option value="round_robin">Round robin</option>
                      <option value="active_standby">Active/standby</option>
                    </Select>
                  </Field>
                  <Field label="TTL (s)">
                    <Input
                      type="number"
                      min={30}
                      value={dns.ttl}
                      onChange={(e) => setDns({ ttl: Number(e.target.value) })}
                    />
                  </Field>
                </div>
              </div>
            )}
          </>
        )}
      </section>
    </div>
  );
}
