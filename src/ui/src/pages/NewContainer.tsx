import { Link, useSearch } from "@tanstack/react-router";
import { ChevronRight, Loader2, Lock, Plus, Rocket, Save, X } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";

import { useServers } from "../api/hooks";
import { type ContainerForm, jobServer, useCreateContainer, useStack } from "../api/stacks";
import { nodeName, type Job } from "../api/types";
import { errorText, useToast } from "../app/toast";
import { NodeJobTabs } from "../components/JobLog";
import { useEnvironmentChoices } from "../components/stacks/CreateStack";
import { YamlEditor, useMfaRetry } from "../components/stacks/parts";
import { ActionBar, Badge, Button, CopyButton, ErrorNote, Field, Input, PageCrumb, PageHeader, Select, Skeleton } from "../components/ui";

const NAME = /^[A-Za-z0-9][A-Za-z0-9_.-]{0,62}$/;
const KEY = /^[A-Za-z_][A-Za-z0-9_]*$/;

interface PortRow {
  container: string;
  host: string;
  protocol: "tcp" | "udp";
  public: boolean;
}
interface KV {
  key: string;
  value: string;
}
interface VolRow {
  source: string;
  target: string;
  read_only: boolean;
}

/** A titled section of the form: bordered card with a header strip and optional note. */
function Section({ title, note, children }: { title: string; note?: ReactNode; children: ReactNode }) {
  return (
    <section className="overflow-hidden rounded-lg border border-line bg-panel shadow-sm">
      <div className="flex min-w-0 flex-wrap items-baseline gap-x-2 border-b border-line bg-panel-2 px-4 py-2">
        <h2 className="text-[13px] font-semibold">{title}</h2>
        {note && <span className="min-w-0 text-[11px] text-faint">{note}</span>}
      </div>
      <div className="space-y-3 p-4">{children}</div>
    </section>
  );
}

/** A labelled list of rows with add/remove buttons; the hint sits with the label, and an empty list says what that means. */
function Rows<T>({ label, hint, empty, rows, onChange, blank, render }: { label: string; hint?: ReactNode; empty: string; rows: T[]; onChange: (r: T[]) => void; blank: T; render: (r: T, set: (p: Partial<T>) => void, i: number) => ReactNode }) {
  return (
    <div className="space-y-1">
      <div className="flex min-w-0 items-start justify-between gap-2">
        <div className="min-w-0">
          <span className="text-xs font-medium text-muted">{label}</span>
          {hint && <p className="text-[11px] text-faint">{hint}</p>}
        </div>
        <button type="button" onClick={() => onChange([...rows, { ...blank }])} className="inline-flex shrink-0 items-center gap-1 rounded border border-line px-1.5 py-0.5 text-xs text-muted hover:bg-panel-2 hover:text-accent">
          <Plus className="size-3.5" /> Add {label.toLowerCase().replace(/s$/, "")}
        </button>
      </div>
      {rows.length === 0 && <p className="rounded-md border border-dashed border-line px-2.5 py-1.5 text-xs text-faint">{empty}</p>}
      {rows.map((r, i) => (
        <div key={i} className="flex min-w-0 flex-wrap items-center gap-1.5 sm:flex-nowrap">
          {render(r, (p) => onChange(rows.map((x, j) => (j === i ? { ...x, ...p } : x))), i)}
          <button type="button" aria-label={`Remove ${label.toLowerCase()} row ${i + 1}`} onClick={() => onChange(rows.filter((_, j) => j !== i))} className="grid size-7 shrink-0 place-items-center rounded text-faint hover:bg-panel-2 hover:text-bad">
            <X className="size-3.5" />
          </button>
        </div>
      ))}
    </div>
  );
}

/** One line of the summary column. */
function Line({ k, children }: { k: string; children: ReactNode }) {
  return (
    <div className="flex min-w-0 gap-2">
      <dt className="w-16 shrink-0 text-faint">{k}</dt>
      <dd className="min-w-0 flex-1 truncate">{children}</dd>
    </div>
  );
}

const small = "py-1 text-xs";

const Crumb = ({ current }: { current: string }) => <PageCrumb back={<Link to="/stacks">Stacks</Link>} current={current} />;

/**
 * New container (`/stacks/new-container`): one image as a compose stack of
 * kind `container`. `?node=` and `?environment=` preselect the target.
 */
export function NewContainer() {
  const search = useSearch({ from: "/stacks/new-container" });
  const toast = useToast();
  const servers = (useServers().data ?? []).filter((s) => s.status !== "revoked");
  const [serverId, setServerId] = useState(search.node ?? "");
  const choices = useEnvironmentChoices(serverId);
  const [ns, setNs] = useState(search.environment ?? "");
  const [name, setName] = useState("");
  const [image, setImage] = useState("");
  const [command, setCommand] = useState("");
  const [entrypoint, setEntrypoint] = useState("");
  const [ports, setPorts] = useState<PortRow[]>([]);
  const [env, setEnv] = useState<KV[]>([]);
  const [volumes, setVolumes] = useState<VolRow[]>([]);
  const [labels, setLabels] = useState<KV[]>([]);
  const [restart, setRestart] = useState<ContainerForm["restart"]>("unless-stopped");
  const [cpus, setCpus] = useState("");
  const [memory, setMemory] = useState("");
  const [network, setNetwork] = useState("");
  const [hcTest, setHcTest] = useState("");
  const [hcInterval, setHcInterval] = useState("30s");
  const [expose, setExpose] = useState<"" | "http" | "https">("");
  const [dir, setDir] = useState("");
  const [start, setStart] = useState(true);
  const [result, setResult] = useState<{ id: string; name: string; jobs: Job[] }>();
  const create = useCreateContainer();
  const mfa = useMfaRetry();

  useEffect(() => {
    if (ns && choices.list.some((n) => n.id === ns)) return;
    const pick = choices.list.find((n) => n.id === choices.server?.environment_id) ?? choices.list[0];
    if (pick) setNs(pick.id);
  }, [choices.list, choices.server, ns]);

  const problems: string[] = [];
  if (name && !NAME.test(name)) problems.push("Name: letters, digits, '.', '_' and '-', starting with a letter or digit.");
  if (ports.some((p) => !/^\d+$/.test(p.container) || Number(p.container) < 1 || Number(p.container) > 65535 || (p.host !== "" && (!/^\d+$/.test(p.host) || Number(p.host) > 65535))))
    problems.push("Ports must be numbers from 1 to 65535.");
  if (env.some((e) => !KEY.test(e.key))) problems.push("Environment keys: letters, digits and _, not starting with a digit.");
  if (volumes.some((v) => !v.source.trim() || !v.target.startsWith("/"))) problems.push("Volumes need a source and an absolute target path.");
  if (cpus && !(Number(cpus) > 0)) problems.push("CPUs must be a positive number.");
  if (memory && !(Number.isInteger(Number(memory)) && Number(memory) > 0)) problems.push("Memory must be a whole number of MiB.");
  if (expose && !ports.some((p) => p.protocol === "tcp")) problems.push("Expose via Nginx needs a TCP port.");

  const ready = !!serverId && !!ns && NAME.test(name) && image.trim().length > 0 && problems.length === 0;

  const submit = async () => {
    const container: ContainerForm = {
      name: name.trim(),
      image: image.trim(),
      command: command.trim() || undefined,
      entrypoint: entrypoint.trim() || undefined,
      ports: ports.map((p) => ({ container: Number(p.container), host: p.host ? Number(p.host) : undefined, protocol: p.protocol, public: p.public })),
      env: env.filter((e) => e.key).map((e) => ({ key: e.key, value: e.value })),
      volumes: volumes.map((v) => ({ source: v.source.trim(), target: v.target.trim(), read_only: v.read_only })),
      restart,
      cpus: cpus ? Number(cpus) : undefined,
      memory_mb: memory ? Number(memory) : undefined,
      network: network.trim() || undefined,
      healthcheck: hcTest.trim() ? { test: hcTest.trim(), interval: hcInterval.trim() || "30s" } : undefined,
      labels: labels.some((l) => l.key) ? Object.fromEntries(labels.filter((l) => l.key).map((l) => [l.key, l.value])) : undefined,
      expose: expose || undefined,
    };
    try {
      const r = await mfa(() => create.mutateAsync({ environment_id: ns, server_id: serverId, dir: dir.trim() || undefined, start, container }));
      toast(start ? `Creating ${r.stack.name}; following the job` : `Saved ${r.stack.name}`);
      setResult({ id: r.stack.id, name: r.stack.name, jobs: r.jobs });
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  const hostOf = (id: string) => {
    const s = servers.find((x) => x.id === id);
    return s ? nodeName(s) : id.slice(0, 8) || "node";
  };

  if (result) return <Created result={result} hostOf={hostOf} />;

  const missing = [!serverId && "node", !ns && "environment", !NAME.test(name) && (name ? "valid name" : "name"), !image.trim() && "image"].filter((x): x is string => !!x);
  const why = missing.length ? `Missing: ${missing.join(", ")}` : problems.length ? "Fix the problems listed above" : undefined;
  const envName = choices.list.find((n) => n.id === ns)?.name;
  const stackName = name.trim().toLowerCase();
  const folder = dir.trim() || `/opt/forge/stacks/${envName ?? "<environment>"}/${stackName || "<name>"}`;
  const advancedSet = labels.length > 0 || !!dir.trim();
  const nodeLabel = serverId ? hostOf(serverId) : undefined;

  return (
    <form
      className="flex min-h-full flex-col"
      onSubmit={(e) => {
        e.preventDefault();
        if (ready) void submit();
      }}
    >
      <Crumb current="New container" />
      <PageHeader title="New container" description="Runs one image as a single-service compose stack. Ports bind to the node's internal address unless marked public." />
      <div className="grid flex-1 items-start gap-6 lg:grid-cols-[minmax(0,1fr)_16rem] xl:grid-cols-[minmax(0,1fr)_18rem]">
        <div className="min-w-0 space-y-4">
          <Section title="Basics" note="Required">
            <div className="grid gap-3 sm:grid-cols-2">
              <Field label="Node">
                <Select value={serverId} onChange={(e) => setServerId(e.target.value)} required>
                  <option value="">Select a node</option>
                  {servers.map((s) => (
                    <option key={s.id} value={s.id}>
                      {nodeName(s)}
                    </option>
                  ))}
                </Select>
              </Field>
              <Field label="Environment" hint="The Forge environment that owns the stack.">
                <Select value={ns} onChange={(e) => setNs(e.target.value)} required>
                  <option value="">Select an environment</option>
                  {choices.list.map((n) => (
                    <option key={n.id} value={n.id}>
                      {n.name} · {choices.clusterName(n.cluster_id)}
                    </option>
                  ))}
                </Select>
              </Field>
              <Field label="Name" hint="Also the stack name (lower-cased).">
                <Input value={name} onChange={(e) => setName(e.target.value)} placeholder="web" maxLength={63} required />
              </Field>
              <Field label="Image">
                <Input value={image} onChange={(e) => setImage(e.target.value)} placeholder="nginx:1.27 or ghcr.io/org/app:tag" className="font-mono text-xs" required />
              </Field>
            </div>
          </Section>

          <Section title="Networking">
            <Rows
              label="Ports"
              rows={ports}
              onChange={setPorts}
              blank={{ container: "", host: "", protocol: "tcp", public: false }}
              hint="host → container. Internal binds to the node's cluster address (or loopback); public listens on every interface."
              empty="No ports published; the container is reachable only on its compose network."
              render={(p, set, i) => (
                <>
                  <Input className={`w-24 ${small}`} inputMode="numeric" placeholder="host" aria-label={`Host port ${i + 1}`} title="Host port (blank: same as container)" value={p.host} onChange={(e) => set({ host: e.target.value })} />
                  <span className="text-faint">→</span>
                  <Input className={`w-24 ${small}`} inputMode="numeric" placeholder="container" aria-label={`Container port ${i + 1}`} value={p.container} onChange={(e) => set({ container: e.target.value })} />
                  <Select className={`w-20 ${small}`} value={p.protocol} onChange={(e) => set({ protocol: e.target.value === "udp" ? "udp" : "tcp" })} aria-label={`Protocol ${i + 1}`}>
                    <option value="tcp">tcp</option>
                    <option value="udp">udp</option>
                  </Select>
                  <div role="radiogroup" aria-label={`Exposure ${i + 1}`} className="flex rounded-md border border-line p-0.5 text-xs">
                    {(["internal", "public"] as const).map((k) => (
                      <button
                        key={k}
                        type="button"
                        role="radio"
                        aria-checked={p.public === (k === "public")}
                        onClick={() => set({ public: k === "public" })}
                        className={`rounded px-2 py-0.5 ${p.public === (k === "public") ? (k === "public" ? "bg-warn-soft text-warn" : "bg-accent-soft text-accent") : "text-muted"}`}
                      >
                        {k}
                      </button>
                    ))}
                  </div>
                  <span className="flex-1" />
                </>
              )}
            />
            <div className="grid gap-3 sm:grid-cols-2">
              <Field label="Expose via Nginx" hint="Marks the first TCP port for a Site; create it later from the stack page.">
                <Select value={expose} onChange={(e) => setExpose(e.target.value as "" | "http" | "https")}>
                  <option value="">Not exposed</option>
                  <option value="http">http</option>
                  <option value="https">https</option>
                </Select>
              </Field>
              <Field label="Network" hint="Blank uses the stack's default network.">
                <Input placeholder="stack default" value={network} onChange={(e) => setNetwork(e.target.value)} className="font-mono text-xs" />
              </Field>
            </div>
          </Section>

          <Section title="Configuration">
            <Rows
              label="Environment variables"
              rows={env}
              onChange={setEnv}
              blank={{ key: "", value: "" }}
              hint={
                <span className="inline-flex items-center gap-1">
                  <Lock className="size-3 shrink-0" /> Stored sealed in the stack's .env and referenced as ${"{KEY}"}; values are not shown again.
                </span>
              }
              empty="No variables; the image defaults apply."
              render={(r, set, i) => (
                <>
                  <Input className={`w-48 font-mono ${small}`} placeholder="KEY" aria-label={`Variable ${i + 1}`} value={r.key} onChange={(e) => set({ key: e.target.value })} />
                  <Input className={`min-w-0 flex-1 font-mono ${small}`} type="password" autoComplete="off" placeholder="value" aria-label={`Value ${i + 1}`} value={r.value} onChange={(e) => set({ value: e.target.value })} />
                </>
              )}
            />
            <Rows
              label="Volumes"
              rows={volumes}
              onChange={setVolumes}
              blank={{ source: "", target: "", read_only: false }}
              hint="A volume name (data) or a host path inside an approved folder → path in the container."
              empty="No volumes; data written in the container is lost when it is recreated."
              render={(v, set, i) => (
                <>
                  <Input className={`min-w-0 flex-1 font-mono ${small}`} placeholder="data or /srv/app" aria-label={`Volume source ${i + 1}`} value={v.source} onChange={(e) => set({ source: e.target.value })} />
                  <span className="text-faint">→</span>
                  <Input className={`min-w-0 flex-1 font-mono ${small}`} placeholder="/var/lib/app" aria-label={`Volume target ${i + 1}`} value={v.target} onChange={(e) => set({ target: e.target.value })} />
                  <label className="flex shrink-0 items-center gap-1 text-xs text-muted">
                    <input type="checkbox" checked={v.read_only} onChange={(e) => set({ read_only: e.target.checked })} /> read-only
                  </label>
                </>
              )}
            />
            <div className="grid gap-3 sm:grid-cols-2">
              <Field label="Command (optional)">
                <Input value={command} onChange={(e) => setCommand(e.target.value)} placeholder="image default" className="font-mono text-xs" />
              </Field>
              <Field label="Entrypoint (optional)">
                <Input value={entrypoint} onChange={(e) => setEntrypoint(e.target.value)} placeholder="image default" className="font-mono text-xs" />
              </Field>
            </div>
          </Section>

          <Section title="Resources & health" note="Blank means no limit and no health check.">
            <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
              <Field label="Restart">
                <Select value={restart} onChange={(e) => setRestart(e.target.value as ContainerForm["restart"])}>
                  <option value="unless-stopped">unless-stopped</option>
                  <option value="always">always</option>
                  <option value="on-failure">on-failure</option>
                  <option value="no">no</option>
                </Select>
              </Field>
              <Field label="CPUs">
                <Input inputMode="decimal" placeholder="no limit" value={cpus} onChange={(e) => setCpus(e.target.value)} className="font-mono text-xs tabular-nums" />
              </Field>
              <Field label="Memory (MiB)">
                <Input inputMode="numeric" placeholder="no limit" value={memory} onChange={(e) => setMemory(e.target.value)} className="font-mono text-xs tabular-nums" />
              </Field>
            </div>
            <div className="grid gap-3 sm:grid-cols-[minmax(0,1fr)_8rem]">
              <Field label="Health check (optional)">
                <Input placeholder="curl -fsS http://localhost/ || exit 1" value={hcTest} onChange={(e) => setHcTest(e.target.value)} className="font-mono text-xs" />
              </Field>
              <Field label="Interval" hint={hcTest.trim() ? undefined : "Set a health check first."}>
                <Input value={hcInterval} onChange={(e) => setHcInterval(e.target.value)} disabled={!hcTest.trim()} className="font-mono text-xs" />
              </Field>
            </div>
          </Section>

          <details open={advancedSet || undefined} className="group overflow-hidden rounded-lg border border-line bg-panel shadow-sm">
            <summary className="flex cursor-pointer list-none items-center gap-1.5 bg-panel-2 px-4 py-2 text-[13px] font-semibold select-none group-open:border-b group-open:border-line">
              <ChevronRight className="size-3.5 text-faint transition-transform group-open:rotate-90" />
              Advanced
              <span className="text-[11px] font-normal text-faint">Labels, folder</span>
            </summary>
            <div className="space-y-3 p-4">
              <Rows
                label="Labels"
                rows={labels}
                onChange={setLabels}
                blank={{ key: "", value: "" }}
                empty="No labels."
                render={(r, set, i) => (
                  <>
                    <Input className={`w-56 font-mono ${small}`} placeholder="com.example.team" aria-label={`Label ${i + 1}`} value={r.key} onChange={(e) => set({ key: e.target.value })} />
                    <Input className={`min-w-0 flex-1 font-mono ${small}`} placeholder="value" aria-label={`Label value ${i + 1}`} value={r.value} onChange={(e) => set({ value: e.target.value })} />
                  </>
                )}
              />
              <Field label="Folder (optional)" hint="Default: /opt/forge/stacks/<environment>/<name>">
                <Input value={dir} onChange={(e) => setDir(e.target.value)} className="font-mono text-xs" />
              </Field>
            </div>
          </details>

          {problems.length > 0 && (
            <ul role="alert" className="space-y-0.5 rounded-md bg-bad-soft px-3 py-2 text-xs text-bad">
              {problems.map((p) => (
                <li key={p}>{p}</li>
              ))}
            </ul>
          )}
        </div>

        <aside aria-label="Summary" className="min-w-0 overflow-hidden rounded-lg border border-line bg-panel shadow-sm lg:sticky lg:top-4">
          <h2 className="border-b border-line bg-panel-2 px-4 py-2 text-[11px] font-semibold tracking-wider text-faint uppercase">Summary</h2>
          <dl className="space-y-1 px-4 py-3 text-xs">
            <Line k="Stack">
              <span className="font-mono text-[11px]" title={stackName || undefined}>
                {stackName || "—"}
              </span>
            </Line>
            <Line k="Node">
              <span title={nodeLabel}>{nodeLabel ?? "—"}</span>
            </Line>
            <Line k="Env">
              <span title={envName}>{envName ?? "—"}</span>
            </Line>
            <Line k="Image">
              <span className="font-mono text-[11px]" title={image.trim() || undefined}>
                {image.trim() || "—"}
              </span>
            </Line>
            <Line k="Folder">
              <span className="font-mono text-[11px]" title={folder}>
                {folder}
              </span>
            </Line>
            <Line k="Ports">
              {ports.length === 0 ? (
                "—"
              ) : (
                <span className="flex flex-wrap gap-1">
                  {ports.map((p, i) => (
                    <Badge key={i} tone={p.public ? "warn" : "muted"}>
                      <span className="font-mono text-[11px] tabular-nums">
                        {p.host || p.container || "?"}→{p.container || "?"}/{p.protocol}
                      </span>
                    </Badge>
                  ))}
                </span>
              )}
            </Line>
            <Line k="Expose">{expose || "—"}</Line>
            <Line k="Env vars">
              <span className="tabular-nums">{env.length || "—"}</span>
            </Line>
            <Line k="Volumes">
              <span className="tabular-nums">{volumes.length || "—"}</span>
            </Line>
            <Line k="Restart">
              <span className="font-mono text-[11px]">{restart}</span>
            </Line>
            <Line k="Limits">
              <span className="font-mono text-[11px] tabular-nums">{cpus || memory ? `${cpus ? `${cpus} CPU` : "—"} · ${memory ? `${memory} MiB` : "—"}` : "—"}</span>
            </Line>
          </dl>
          {missing.length > 0 && <p className="mx-4 mb-3 text-[11px] text-warn">Still needed: {missing.join(", ")}.</p>}
        </aside>
      </div>

      <ActionBar note={why ? <span className="text-warn">{why}</span> : "Forge saves it as a compose stack; edit it later like any other stack."}>
        <label className="flex items-center gap-1.5 text-[13px]" title="Unticked: save the stack without deploying it.">
          <input type="checkbox" checked={start} onChange={(e) => setStart(e.target.checked)} /> Start now
        </label>
        <Link to="/stacks">
          <Button type="button" variant="ghost">
            Cancel
          </Button>
        </Link>
        <span title={why} className="inline-flex">
          <Button variant="primary" disabled={!ready || create.isPending}>
            {create.isPending ? <Loader2 className="size-4 animate-spin" /> : start ? <Rocket className="size-4" /> : <Save className="size-4" />}
            {start ? "Create and start" : "Create"}
          </Button>
        </span>
      </ActionBar>
    </form>
  );
}

/** After creating: the generated compose file and the deployment jobs. */
function Created({ result, hostOf }: { result: { id: string; name: string; jobs: Job[] }; hostOf: (id: string) => string }) {
  const stack = useStack(result.id);
  const content = stack.data?.content ?? "";
  return (
    <>
      <Crumb current={result.name} />
      <PageHeader
        title={`Container ${result.name}`}
        description="Forge saved it as a compose stack. This is the file it generated."
        actions={
          <Link to="/stacks/$stackId" params={{ stackId: result.id }}>
            <Button variant="primary">Open stack</Button>
          </Link>
        }
      />
      <div className="max-w-5xl space-y-4">
        <div>
          <div className="mb-1 flex items-center justify-between">
            <span className="font-mono text-xs text-muted">compose.yaml · revision {stack.data?.stack.revision ?? "—"}</span>
            {content && <CopyButton text={content} />}
          </div>
          {stack.isLoading ? (
            <Skeleton rows={4} />
          ) : stack.error ? (
            <ErrorNote error={stack.error} />
          ) : (
            <YamlEditor value={content} docKey={`gen-${result.id}-${content.length}`} onChange={() => {}} readOnly height="h-56" />
          )}
        </div>
        {result.jobs.length > 0 ? (
          <NodeJobTabs jobs={result.jobs.map((j) => ({ nodeId: jobServer(j) || j.id, nodeName: hostOf(jobServer(j)), jobId: j.id }))} />
        ) : (
          <p className="text-xs text-muted">Not started. Deploy it from the stack page.</p>
        )}
      </div>
    </>
  );
}
