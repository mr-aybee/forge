import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { Globe, Loader2, Lock, RefreshCw, Rocket, Save, Trash2 } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { ApiError, get } from "../../api/client";
import { useServers } from "../../api/hooks";
import {
  type Preview,
  type StackDetail,
  type StackNode,
  previewStack,
  stackLogs,
  useDeployStack,
  useDownStack,
  useExposeStack,
  useUpdateStack,
} from "../../api/stacks";
import { nodeName, type Job } from "../../api/types";
import { errorText, useToast } from "../../app/toast";
import type { Certificate } from "../ingress/types";
import { Badge, Button, CopyButton, Field, Input, Modal, Select } from "../ui";
import { Designer, MeshToggle, ServicePicker, YamlEditor, startBlocker, useMfaRetry } from "./parts";
import { DiffView } from "./Revisions";
import { CodeField } from "../CodeField";

type Done = (jobs: Job[]) => void;

/** Deploy (or roll back): pick services to start, optionally pull images. */
export function DeployDialog({ d, revision, onClose, onJobs }: { d: StackDetail; revision?: number; onClose: () => void; onJobs: Done }) {
  const toast = useToast();
  const deploy = useDeployStack();
  const mfa = useMfaRetry();
  const services = d.preview?.services ?? [];
  const [ticked, setTicked] = useState<Set<string>>(() => new Set(d.nodes.flatMap((n) => n.selected ?? []).filter((x) => services.some((s) => s.name === x))));
  const [pull, setPull] = useState(false);
  const blocker = revision ? null : startBlocker(d.preview);
  const rollback = !!revision && revision !== d.stack.revision;
  const nodes = d.preview?.nodes ?? [];
  return (
    <Modal open onOpenChange={(o) => !o && onClose()} title={rollback ? `Roll back ${d.stack.name} to r${revision}` : `Deploy ${d.stack.name}`} description={rollback ? undefined : `Revision r${d.stack.revision}${d.stack.deployed_revision ? ` (deployed: r${d.stack.deployed_revision})` : " (never deployed)"}`} wide>
      <div className="space-y-3">
        <div className="rounded-md border border-line px-3 py-2 text-xs">
          <div className="mb-1 font-medium">What happens</div>
          <ol className="list-decimal space-y-0.5 pl-4 text-muted">
            <li>Forge writes compose.yaml (r{revision ?? d.stack.revision}) and the sealed .env to {d.dir ? <span className="font-mono">{d.dir}</span> : "the stack folder"} on {nodes.length || "each"} node{nodes.length === 1 ? "" : "s"}.</li>
            <li>{pull ? "Images are pulled again, even when present." : "Missing images are pulled."}</li>
            <li>
              <span className="font-mono">docker compose up -d</span> starts {ticked.size ? "the ticked services and their dependencies" : "every service"}; changed services are recreated (a short outage for them).
            </li>
            <li>Nodes that no longer run part of the stack take their part down.</li>
          </ol>
        </div>
        {rollback && <p className="rounded-md bg-warn-soft px-3 py-2 text-xs text-warn">Rolling back deploys the older file; the latest revision stays r{d.stack.revision}. Update the stack to make it permanent.</p>}
        {!rollback && <ServicePicker services={services} ticked={ticked} onChange={setTicked} />}
        {!d.preview && <p className="text-xs text-bad">{d.preview_error ?? "The stack has no valid preview; Update it first."}</p>}
        <label className="flex items-center gap-1.5 text-[13px]">
          <input type="checkbox" checked={pull} onChange={(e) => setPull(e.target.checked)} /> Pull images again (<span className="font-mono text-xs">--pull always</span>)
        </label>
        {blocker && <p className={`rounded-md px-3 py-2 text-xs ${d.preview?.blocked ? "bg-bad-soft text-bad" : "bg-warn-soft text-warn"}`}>{blocker}</p>}
        {d.preview?.privileged && !blocker && <p className="rounded-md bg-warn-soft px-3 py-2 text-xs text-warn">This stack needs privileged access; you may be asked to verify with MFA.</p>}
        <div className="flex justify-end gap-2">
          <Button onClick={onClose}>Cancel</Button>
          <Button
            variant="primary"
            disabled={deploy.isPending || !!blocker || (!rollback && !d.preview)}
            title={blocker ?? (!rollback && !d.preview ? "The compose file has no valid preview" : undefined)}
            onClick={() =>
              void mfa(() => deploy.mutateAsync({ id: d.stack.id, revision: rollback ? revision : undefined, services: !rollback && ticked.size ? [...ticked] : undefined, pull: pull || undefined }))
                .then((jobs) => {
                  toast(`${rollback ? "Rolling back" : "Deploying"} on ${jobs.length} node${jobs.length === 1 ? "" : "s"}`);
                  onJobs(jobs);
                  onClose();
                })
                .catch((e) => toast(errorText(e), "error"))
            }
          >
            {deploy.isPending ? <Loader2 className="size-4 animate-spin" /> : <Rocket className="size-4" />}
            {rollback ? `Deploy r${revision}` : "Deploy"}
          </Button>
        </div>
      </div>
    </Modal>
  );
}

/** Update: edit compose + .env (blank keeps), saved as a new revision. */
export function UpdateDialog({ d, onClose, onSaved }: { d: StackDetail; onClose: () => void; onSaved: () => void }) {
  const toast = useToast();
  const update = useUpdateStack();
  const [content, setContent] = useState(d.content);
  const [env, setEnv] = useState("");
  const [dir, setDir] = useState(d.stack.dir ?? "");
  const [note, setNote] = useState("");
  const [preview, setPreview] = useState<Preview | null>(d.preview);
  const [pvError, setPvError] = useState<string>();
  const [view, setView] = useState<"edit" | "diff" | "preview">("edit");
  const [docRev, setDocRev] = useState(0);
  const changed = content !== d.content;

  useEffect(() => {
    if (!changed && !env) return;
    const t = setTimeout(() => {
      previewStack({ environment_id: d.stack.environment_id, server_id: d.stack.default_server, name: d.stack.name, content, env: env || undefined, dir: dir.trim() || undefined })
        .then((p) => {
          setPreview(p);
          setPvError(undefined);
        })
        .catch((e) => setPvError(errorText(e)));
    }, 900);
    return () => clearTimeout(t);
  }, [content, env, dir, changed, d.stack]);

  const nothing = !changed && !env && (dir.trim() || null) === (d.stack.dir ?? null) && !note.trim();
  return (
    <Modal open onOpenChange={(o) => !o && onClose()} title={`Update ${d.stack.name}`} description={`Saving a changed file creates revision r${d.stack.revision + 1}. Nothing changes on the nodes until you deploy.`} width={980}>
      <div className="space-y-3">
        <div className="flex items-center gap-1 border-b border-line">
          {(["edit", "diff", "preview"] as const).map((v) => (
            <button key={v} type="button" onClick={() => setView(v)} className={`-mb-px border-b-2 px-3 py-1.5 text-[13px] capitalize ${view === v ? "border-accent text-accent" : "border-transparent text-muted hover:text-fg"}`}>
              {v === "diff" ? `Changes${changed ? "" : " (none)"}` : v}
            </button>
          ))}
          <span className="ml-auto pr-1">
            <MeshToggle
              content={content}
              onChange={(t) => {
                setContent(t);
                setDocRev((k) => k + 1);
              }}
            />
          </span>
        </div>
        {view === "edit" && <YamlEditor value={content} docKey={`upd-${d.stack.id}-${d.stack.revision}-${docRev}`} onChange={setContent} height="h-[42vh]" />}
        {view === "diff" && <DiffView before={d.content} after={content} />}
        {view === "preview" && (preview ? <div className="max-h-[42vh] overflow-y-auto"><Designer preview={preview} /></div> : <p className="text-xs text-muted">No preview.</p>)}
        {pvError && <p role="alert" className="rounded-md bg-bad-soft px-3 py-2 font-mono text-[11.5px] whitespace-pre-wrap text-bad">{pvError}</p>}
        <div className="grid gap-3 sm:grid-cols-2">
          <Field
            label=".env (blank keeps the stored values)"
            hint={
              <span className="inline-flex flex-wrap items-center gap-1">
                <Lock className="size-3" /> Stored sealed.{" "}
                {d.stack.env_keys.length ? (
                  <>
                    Now set: <span className="font-mono">{d.stack.env_keys.join(", ")}</span>
                  </>
                ) : (
                  "No variables stored."
                )}
              </span>
            }
          >
            <CodeField lang="env" value={env} onChange={setEnv} placeholder="KEY=value (replaces every stored variable)" />
          </Field>
          <div className="space-y-3">
            <Field label="Folder" hint="Blank: /opt/forge/stacks/<environment>/<stack>">
              <Input value={dir} onChange={(e) => setDir(e.target.value)} className="font-mono text-xs" />
            </Field>
            <Field label="Note (optional)">
              <Input value={note} onChange={(e) => setNote(e.target.value)} placeholder="Bump postgres to 16.4" maxLength={200} />
            </Field>
          </div>
        </div>
        <div className="flex items-center justify-end gap-2">
          {preview?.blocked && <span className="mr-auto text-xs text-bad">A service is blocked by policy; it can be saved but not deployed.</span>}
          <Button onClick={onClose}>Cancel</Button>
          <Button
            variant="primary"
            disabled={update.isPending || nothing}
            title={nothing ? "Nothing changed" : undefined}
            onClick={() =>
              update.mutate(
                {
                  id: d.stack.id,
                  content: changed ? content : undefined,
                  env: env || undefined,
                  dir: dir.trim() || undefined,
                  note: note.trim() || undefined,
                  version: d.stack.updated_at,
                },
                {
                  onSuccess: (r) => {
                    toast(r.stack && r.stack.revision !== d.stack.revision ? `Saved as r${r.stack.revision}; deploy to apply it` : "Saved");
                    onSaved();
                    onClose();
                  },
                  onError: (e) =>
                    toast(
                      e instanceof ApiError && e.code === "stale_version"
                        ? "Someone changed this stack since you opened it. Copy your edits, reopen Update and apply them again."
                        : errorText(e),
                      "error",
                    ),
                },
              )
            }
          >
            {update.isPending ? <Loader2 className="size-4 animate-spin" /> : <Save className="size-4" />} Save
          </Button>
        </div>
      </div>
    </Modal>
  );
}

/** Down: stops and removes the containers; volumes only when ticked. */
export function DownDialog({ d, node, onClose, onJobs }: { d: StackDetail; node?: StackNode; onClose: () => void; onJobs: Done }) {
  const toast = useToast();
  const down = useDownStack();
  const servers = useServers().data ?? [];
  const [volumes, setVolumes] = useState(false);
  const host = (id: string) => {
    const s = servers.find((x) => x.id === id);
    return s ? nodeName(s) : id.slice(0, 8);
  };
  const targets = node ? [node] : d.nodes;
  return (
    <Modal open onOpenChange={(o) => !o && onClose()} title={`Take ${d.stack.name} down${node ? ` on ${host(node.server_id)}` : ""}?`}>
      <div className="space-y-3 text-[13px]">
        <p className="text-muted">
          Runs <span className="font-mono text-xs">docker compose down</span> on {targets.map((n) => host(n.server_id)).join(", ") || "every node"}. The services ({[...new Set(targets.flatMap((n) => n.services))].join(", ") || "all"}) stop and their containers are removed; anything that depends on them loses them. The stack and its revisions stay, so you can deploy it again.
        </p>
        <label className={`flex items-start gap-2 rounded-md border px-3 py-2 ${volumes ? "border-bad/50 bg-bad-soft text-bad" : "border-line"}`}>
          <input type="checkbox" className="mt-0.5" checked={volumes} onChange={(e) => setVolumes(e.target.checked)} />
          <span>
            <b>Also remove volumes (data is lost)</b>
            <span className="block text-xs opacity-80">Deletes the stack's named volumes. Bind-mounted host folders are kept.</span>
          </span>
        </label>
        <div className="flex justify-end gap-2">
          <Button onClick={onClose}>Cancel</Button>
          <Button
            variant="danger"
            disabled={down.isPending}
            onClick={() =>
              down.mutate(
                { id: d.stack.id, volumes, server_id: node?.server_id },
                {
                  onSuccess: (jobs) => {
                    toast(`Taking ${d.stack.name} down${volumes ? " with its volumes" : ""}`);
                    onJobs(jobs);
                    onClose();
                  },
                  onError: (e) => toast(errorText(e), "error"),
                },
              )
            }
          >
            {down.isPending ? <Loader2 className="size-4 animate-spin" /> : <Trash2 className="size-4" />} {volumes ? "Down and delete data" : "Down"}
          </Button>
        </div>
      </div>
    </Modal>
  );
}

/** Expose a service's published port through Nginx: creates a Site. */
export function ExposeDialog({ d, onClose }: { d: StackDetail; onClose: () => void }) {
  const toast = useToast();
  const expose = useExposeStack();
  const servers = (useServers().data ?? []).filter((s) => s.status !== "revoked");
  const certs = useQuery({ queryKey: ["certificates"], queryFn: () => get<Certificate[]>("/api/v1/certificates").then((r) => (Array.isArray(r) ? r : [])).catch(() => [] as Certificate[]), staleTime: 60_000 }).data ?? [];
  const candidates = (d.preview?.services ?? []).filter((s) => s.ports.some((p) => p.protocol === "tcp" && /^\d+$/.test(p.published)));
  const [service, setService] = useState(candidates.find((c) => c.expose)?.name ?? candidates[0]?.name ?? "");
  const svc = candidates.find((c) => c.name === service);
  const ports = (svc?.ports ?? []).filter((p) => p.protocol === "tcp" && /^\d+$/.test(p.published));
  const [port, setPort] = useState("");
  const [domains, setDomains] = useState("");
  const [tls, setTls] = useState<"off" | "acme" | "uploaded">(svc?.expose === "https" ? "acme" : "off");
  const [email, setEmail] = useState("");
  const [cert, setCert] = useState("");
  const [edges, setEdges] = useState<Set<string>>(() => new Set(svc?.nodes.map((n) => n.server_id) ?? []));
  const [created, setCreated] = useState<string>();

  useEffect(() => {
    if (!ports.some((p) => p.published === port)) setPort(ports[0]?.published ?? "");
  }, [ports, port]);

  const list = domains.split(/[\s,]+/).map((x) => x.trim().toLowerCase()).filter(Boolean);
  const badDomain = list.find((x) => !/^(\*\.)?[a-z0-9-]+(\.[a-z0-9-]+)+$/.test(x));
  const ready = !!svc && !!port && list.length > 0 && !badDomain && edges.size > 0 && (tls !== "acme" || /.+@.+/.test(email)) && (tls !== "uploaded" || !!cert);

  if (created)
    return (
      <Modal open onOpenChange={(o) => !o && onClose()} title="Site created">
        <p className="text-[13px] text-muted">{created}</p>
        <div className="mt-4 flex justify-end gap-2">
          <Button onClick={onClose}>Close</Button>
          <Link to="/ingress" onClick={onClose}>
            <Button variant="primary">
              <Globe className="size-4" /> Open Ingress
            </Button>
          </Link>
        </div>
      </Modal>
    );

  return (
    <Modal open onOpenChange={(o) => !o && onClose()} title="Expose via Nginx" description="Creates a Site that proxies a domain to the service's published port. Nothing changes on the edges until you apply the Site." wide>
      {candidates.length === 0 ? (
        <p className="text-[13px] text-muted">No service publishes a TCP port. Add a port mapping (Update) and deploy first.</p>
      ) : (
        <form
          className="space-y-3"
          onSubmit={(e) => {
            e.preventDefault();
            if (!ready) return;
            expose.mutate(
              { id: d.stack.id, service, port: Number(port), domains: list, tls, acme_email: tls === "acme" ? email : undefined, cert_id: tls === "uploaded" ? cert : undefined, edges: [...edges] },
              {
                onSuccess: (r) => {
                  toast("Site created");
                  setCreated(r.next || "Apply the site on its edges from Ingress → Endpoints.");
                },
                onError: (err) => toast(errorText(err), "error"),
              },
            );
          }}
        >
          <div className="grid gap-3 sm:grid-cols-2">
            <Field label="Service">
              <Select value={service} onChange={(e) => setService(e.target.value)}>
                {candidates.map((c) => (
                  <option key={c.name} value={c.name}>
                    {c.name}
                  </option>
                ))}
              </Select>
            </Field>
            <Field label="Port" hint="The service's published (host) port.">
              <Select value={port} onChange={(e) => setPort(e.target.value)}>
                {ports.map((p) => (
                  <option key={`${p.host_ip}:${p.published}`} value={p.published}>
                    {p.host_ip ? `${p.host_ip}:` : ""}
                    {p.published} → {p.target}
                  </option>
                ))}
              </Select>
            </Field>
          </div>
          <Field label="Domains" hint={badDomain ? <span className="text-bad">“{badDomain}” is not a valid domain.</span> : "One or more, separated by commas or spaces."}>
            <Input value={domains} onChange={(e) => setDomains(e.target.value)} placeholder="app.example.com" className="font-mono text-xs" required />
          </Field>
          <div className="grid gap-3 sm:grid-cols-2">
            <Field label="TLS">
              <Select value={tls} onChange={(e) => setTls(e.target.value as "off" | "acme" | "uploaded")}>
                <option value="off">Off (HTTP only)</option>
                <option value="acme">ACME (Let's Encrypt)</option>
                <option value="uploaded">Uploaded certificate</option>
              </Select>
            </Field>
            {tls === "acme" && (
              <Field label="ACME e-mail">
                <Input type="email" value={email} onChange={(e) => setEmail(e.target.value)} placeholder="ops@example.com" required />
              </Field>
            )}
            {tls === "uploaded" && (
              <Field label="Certificate" hint={certs.length ? undefined : "No certificates uploaded; add one under Ingress → Certificates."}>
                <Select value={cert} onChange={(e) => setCert(e.target.value)} required>
                  <option value="">Select a certificate</option>
                  {certs.map((c) => (
                    <option key={c.id} value={c.id}>
                      {c.name} · {(c.domains ?? []).join(", ")}
                    </option>
                  ))}
                </Select>
              </Field>
            )}
          </div>
          <fieldset className="space-y-1">
            <legend className="text-xs font-medium text-muted">Edges (nodes running Nginx)</legend>
            <div className="flex flex-wrap gap-1.5">
              {servers.map((s) => {
                const on = edges.has(s.id);
                return (
                  <label key={s.id} className={`inline-flex cursor-pointer items-center gap-1.5 rounded-full border px-2.5 py-0.5 text-xs ${on ? "border-accent bg-accent-soft text-accent" : "border-line text-muted hover:border-accent/60"}`}>
                    <input
                      type="checkbox"
                      className="sr-only"
                      checked={on}
                      onChange={() =>
                        setEdges((cur) => {
                          const n = new Set(cur);
                          if (n.has(s.id)) n.delete(s.id);
                          else n.add(s.id);
                          return n;
                        })
                      }
                    />
                    {nodeName(s)}
                    {svc?.nodes.some((n) => n.server_id === s.id) && <span className="text-[10px] opacity-70">runs it</span>}
                  </label>
                );
              })}
            </div>
            <p className="text-[11px] text-faint">Without a cluster network the service listens on loopback, so the edge must be the service's own node.</p>
          </fieldset>
          <div className="flex justify-end gap-2">
            <Button type="button" onClick={onClose}>
              Cancel
            </Button>
            <Button variant="primary" disabled={!ready || expose.isPending}>
              {expose.isPending ? <Loader2 className="size-4 animate-spin" /> : <Globe className="size-4" />} Create Site
            </Button>
          </div>
        </form>
      )}
    </Modal>
  );
}

/** Log tail of one service, with node switching where it runs on several. */
export function LogsDialog({ d, service, server, onClose }: { d: StackDetail; service: string; server: string; onClose: () => void }) {
  const servers = useServers().data ?? [];
  const nodes = d.nodes.filter((n) => n.services.includes(service));
  const [node, setNode] = useState(server);
  const [tail, setTail] = useState(200);
  const [text, setText] = useState<string>();
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
  const [tick, setTick] = useState(0);
  const box = useRef<HTMLPreElement>(null);

  useEffect(() => {
    const sig = { stop: false };
    setBusy(true);
    setError(undefined);
    stackLogs(d.stack.id, node, service, tail, sig)
      .then((t) => !sig.stop && setText(t))
      .catch((e) => !sig.stop && setError(errorText(e)))
      .finally(() => !sig.stop && setBusy(false));
    return () => {
      sig.stop = true;
    };
  }, [d.stack.id, node, service, tail, tick]);

  useEffect(() => {
    if (box.current) box.current.scrollTop = box.current.scrollHeight;
  }, [text]);

  const host = (id: string) => {
    const s = servers.find((x) => x.id === id);
    return s ? nodeName(s) : id.slice(0, 8);
  };
  return (
    <Modal open onOpenChange={(o) => !o && onClose()} title={`Logs · ${service}`} description={`${d.stack.name} on ${host(node)}`} width={960}>
      <div className="mb-2 flex flex-wrap items-center gap-2">
        {nodes.length > 1 && (
          <div role="tablist" aria-label="Node" className="flex rounded-md border border-line p-0.5 text-xs">
            {nodes.map((n) => (
              <button key={n.server_id} role="tab" aria-selected={n.server_id === node} onClick={() => setNode(n.server_id)} className={`rounded px-2.5 py-1 ${n.server_id === node ? "bg-accent-soft text-accent" : "text-muted"}`}>
                {host(n.server_id)}
              </button>
            ))}
          </div>
        )}
        <Select className="w-auto" value={tail} onChange={(e) => setTail(Number(e.target.value))} aria-label="Lines">
          {[100, 200, 500, 2000, 10000].map((x) => (
            <option key={x} value={x}>
              last {x} lines
            </option>
          ))}
        </Select>
        <Button onClick={() => setTick((t) => t + 1)} disabled={busy} className="h-8">
          <RefreshCw className={`size-4 ${busy ? "animate-spin" : ""}`} /> Refresh
        </Button>
        <span className="flex-1" />
        {text && <CopyButton text={text} />}
      </div>
      {error && <p className="mb-2 rounded-md bg-bad-soft px-3 py-2 text-xs text-bad">{error}</p>}
      <pre ref={box} className="log-console h-[55vh] overflow-auto rounded-lg bg-term p-3 font-mono text-[11.5px] leading-relaxed whitespace-pre text-term-fg select-text">
        {busy && text === undefined ? "Reading logs from the node…" : text || "No output."}
      </pre>
    </Modal>
  );
}

export function KindBadge({ kind }: { kind: string }) {
  return <Badge tone={kind === "container" ? "muted" : "accent"}>{kind === "container" ? "container" : "compose"}</Badge>;
}
