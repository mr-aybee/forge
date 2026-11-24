import { useQueryClient } from "@tanstack/react-query";
import { blankFabric, FabricForm, type FabricSpec, validSubnet } from "../fabric/FabricForm";
import { Check, CheckCircle2, CircleDashed, Loader2, XCircle } from "lucide-react";
import { useEffect, useState } from "react";

import { api, post } from "../../api/client";
import { useInstallations, useServers } from "../../api/hooks";
import { type Cluster, nodeName } from "../../api/types";
import { errorText } from "../../app/toast";
import { type ClusterProfile, defaultNginx, type IngressMode, MODES } from "../ingress/types";
import { Badge, Button, Field, Input, Modal } from "../ui";

const STEPS = ["Cluster", "Network", "Ingress", "Review"] as const;
type Log = { label: string; state: "todo" | "run" | "ok" | "fail"; error?: string };

/**
 * First-run and "new cluster" setup: members, the Forge cluster network and
 * the Nginx ingress layout; then installs what is missing and saves the profile.
 */
export function ClusterWizard({ open, onOpenChange, onDone }: { open: boolean; onOpenChange: (o: boolean) => void; onDone?: (clusterId: string) => void }) {
  const qc = useQueryClient();
  const nodes = (useServers().data ?? []).filter((n) => n.status !== "revoked");
  const installs = useInstallations().data ?? [];
  const [step, setStep] = useState(0);
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [memberIds, setMemberIds] = useState<string[]>([]);
  const [withNetwork, setWithNetwork] = useState(true);
  const [fabric, setFabric] = useState<FabricSpec>(blankFabric());
  const [mode, setMode] = useState<IngressMode | "skip">("standalone");
  const [edges, setEdges] = useState<{ server_id: string; role: "primary" | "backup" }[]>([]);
  const [log, setLog] = useState<Log[]>([]);
  const [running, setRunning] = useState(false);

  useEffect(() => {
    if (!open) return;
    setStep(0);
    setName("");
    setDescription("");
    setMemberIds([]);
    setWithNetwork(true);
    setFabric(blankFabric());
    setMode("standalone");
    setEdges([]);
    setLog([]);
    setRunning(false);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  // The control node takes part in every cluster.
  const scope = nodes.filter((n) => memberIds.includes(n.id) || n.role === "control");
  const has = (server: string, pkg: string) => installs.some((i) => i.server_id === server && i.package === pkg && i.status === "installed");
  // HA layouts fail over through DNS (Ingress → Profile → Traffic & DNS); a
  // same-LAN virtual IP stays an advanced option there.
  const profile = (): ClusterProfile => ({
    ingress:
      mode === "skip"
        ? null
        : {
            mode,
            edges,
            failover: "dns",
            vrrp: null,
            nginx: defaultNginx(),
          },
  });

  const plan = (): { label: string; run: (ctx: { id: string }) => Promise<unknown> }[] => {
    const out: { label: string; run: (ctx: { id: string }) => Promise<unknown> }[] = [];
    const install = (pkg: string, version: string, ids: string[], params: Record<string, string> = {}) => {
      const todo = ids.filter((id) => !has(id, pkg));
      if (todo.length)
        out.push({
          label: `Install ${pkg} on ${todo.map((id) => nodeName(nodes.find((n) => n.id === id)!)).join(", ")}`,
          run: () => post("/api/v1/packages/install", { package: pkg, version, server_ids: todo, params }),
        });
    };
    for (const id of memberIds) {
      const n = nodes.find((x) => x.id === id);
      if (n && n.role !== "control") out.push({ label: `Add ${nodeName(n)} to the cluster`, run: (c) => api("PATCH", `/api/v1/servers/${id}`, { cluster_id: c.id }) });
    }
    if (withNetwork) out.push({ label: `Create the network ${fabric.subnet}`, run: (c) => api("PUT", `/api/v1/clusters/${c.id}/fabric`, { spec: fabric, pins: [] }) });
    if (mode !== "skip") {
      install("nginx", "stable", edges.map((e) => e.server_id));
    }
    out.push({ label: "Save the cluster profile", run: (c) => api("PUT", `/api/v1/clusters/${c.id}/profile`, profile()) });
    return out;
  };

  const valid = [
    name.trim().length > 0,
    !withNetwork || (validSubnet(fabric.subnet) && !!fabric.domain),
    mode === "skip" || (mode === "standalone" ? edges.length === 1 : edges.length >= 2 && (mode !== "active_standby" || edges.filter((e) => e.role === "primary").length === 1)),
    true,
  ];

  const run = async () => {
    setRunning(true);
    const steps = plan();
    const entries: Log[] = [{ label: `Create cluster “${name.trim()}”`, state: "run" }, ...steps.map((s) => ({ label: s.label, state: "todo" as const }))];
    setLog([...entries]);
    const mark = (i: number, state: Log["state"], error?: string) => {
      entries[i] = { ...entries[i]!, state, error };
      setLog([...entries]);
    };
    let id = "";
    try {
      const c = await post<Cluster>("/api/v1/clusters", { name: name.trim(), description, mode: "default" });
      id = c.id;
      mark(0, "ok");
    } catch (e) {
      mark(0, "fail", errorText(e));
      setRunning(false);
      return;
    }
    for (const [i, s] of steps.entries()) {
      mark(i + 1, "run");
      try {
        await s.run({ id });
        mark(i + 1, "ok");
      } catch (e) {
        mark(i + 1, "fail", errorText(e));
      }
    }
    for (const k of ["clusters", "servers", "installations", "jobs"]) void qc.invalidateQueries({ queryKey: [k] });
    setRunning(false);
    onDone?.(id);
  };

  const done = log.length > 0 && !running;

  return (
    <Modal open={open} onOpenChange={(o) => !running && onOpenChange(o)} title="Set up a cluster" description="Members, the cluster network and the Nginx ingress. You can change any of it later." width={960}>
      <ol className="mb-5 flex items-center gap-1 text-xs">
        {STEPS.map((s, i) => (
          <li key={s} className="flex flex-1 items-center gap-1">
            <button
              type="button"
              disabled={log.length > 0 || i > step}
              onClick={() => setStep(i)}
              className={`flex items-center gap-1.5 rounded-full px-2 py-1 ${i === step ? "bg-accent-soft font-medium text-accent" : i < step ? "text-fg" : "text-faint"}`}
            >
              <span className={`grid size-5 place-items-center rounded-full border text-[10px] ${i < step ? "border-ok bg-ok text-bg" : i === step ? "border-accent" : "border-line"}`}>
                {i < step ? <Check className="size-3" /> : i + 1}
              </span>
              {s}
            </button>
            {i < STEPS.length - 1 && <span className="h-px flex-1 bg-line" />}
          </li>
        ))}
      </ol>

      {log.length > 0 ? (
        <div className="space-y-2">
          {log.map((l, i) => (
            <div key={i} className="flex items-start gap-2 text-[13px]">
              {l.state === "ok" ? (
                <CheckCircle2 className="mt-0.5 size-4 text-ok" />
              ) : l.state === "fail" ? (
                <XCircle className="mt-0.5 size-4 text-bad" />
              ) : l.state === "run" ? (
                <Loader2 className="mt-0.5 size-4 animate-spin text-accent" />
              ) : (
                <CircleDashed className="mt-0.5 size-4 text-faint" />
              )}
              <div>
                <div>{l.label}</div>
                {l.error && <div className="text-xs text-bad">{l.error}</div>}
              </div>
            </div>
          ))}
          {done && (
            <p className="mt-3 rounded-lg border border-line bg-panel-2/40 p-3 text-xs text-muted">
              Package installs run as jobs on each node (see Jobs). When they finish, open <b>Ingress → Profile</b> and press <b>Save &amp; apply</b> to configure Nginx on the edges.
            </p>
          )}
        </div>
      ) : (
        <div className="min-h-[280px] space-y-3">
          {step === 0 && (
            <>
              <div className="grid gap-3 sm:grid-cols-2">
                <Field label="Name">
                  <Input value={name} onChange={(e) => setName(e.target.value)} placeholder="production" autoFocus />
                </Field>
                <Field label="Description">
                  <Input value={description} onChange={(e) => setDescription(e.target.value)} placeholder="EU web tier" />
                </Field>
              </div>
              <Field label="Members" hint="The control node takes part in every cluster.">
                <div className="max-h-56 divide-y divide-line overflow-y-auto rounded-lg border border-line">
                  {nodes.map((n) => {
                    const isCtl = n.role === "control";
                    const on = isCtl || memberIds.includes(n.id);
                    return (
                      <label key={n.id} className={`flex items-center gap-3 px-3 py-2 ${isCtl ? "opacity-70" : "cursor-pointer"}`}>
                        <input type="checkbox" disabled={isCtl} checked={on} onChange={() => setMemberIds(on ? memberIds.filter((x) => x !== n.id) : [...memberIds, n.id])} />
                        <span className="flex-1 font-medium">{nodeName(n)}</span>
                        {isCtl && <Badge tone="accent">control</Badge>}
                        {n.cluster_id && !isCtl && <Badge tone="warn">in another cluster</Badge>}
                        <span className="text-xs text-faint">
                          {n.os}/{n.arch} · {n.status}
                        </span>
                      </label>
                    );
                  })}
                </div>
              </Field>
            </>
          )}

          {step === 1 && (
            <>
              <label className="flex items-center gap-2 text-[13px]">
                <input type="checkbox" checked={withNetwork} onChange={(e) => setWithNetwork(e.target.checked)} /> Create the Forge cluster network now (addresses, internal DNS, encrypted mesh)
              </label>
              {withNetwork ? <FabricForm value={fabric} onChange={setFabric} clusterName={name} /> : <p className="text-[13px] text-muted">Set it up later from the cluster's Network page.</p>}
            </>
          )}

          {step === 2 && (
            <>
              <div className="grid gap-2 sm:grid-cols-4">
                {[...MODES, { id: "skip" as const, title: "Later", body: "Set up the ingress afterwards.", min: 0 }].map((m) => (
                  <button
                    key={m.id}
                    type="button"
                    onClick={() => {
                      setMode(m.id);
                      if (m.id === "standalone") setEdges(edges.slice(0, 1).map((e) => ({ ...e, role: "primary" })));
                    }}
                    className={`rounded-lg border p-3 text-left ${mode === m.id ? "border-accent bg-accent-soft" : "border-line hover:border-accent/40"}`}
                  >
                    <div className="font-medium">{m.title}</div>
                    <p className="mt-1 text-xs text-muted">{m.body}</p>
                  </button>
                ))}
              </div>
              {mode !== "skip" && (
                <>
                  <Field label="Edge nodes" hint={mode === "standalone" ? "Exactly one." : mode === "active_standby" ? "The first one you pick is the primary." : "Two or more."}>
                    <div className="flex flex-wrap gap-1.5">
                      {scope.map((n) => {
                        const e = edges.find((x) => x.server_id === n.id);
                        return (
                          <button
                            key={n.id}
                            type="button"
                            onClick={() => {
                              if (e) setEdges(edges.filter((x) => x.server_id !== n.id));
                              else if (mode === "standalone") setEdges([{ server_id: n.id, role: "primary" }]);
                              else setEdges([...edges, { server_id: n.id, role: edges.some((x) => x.role === "primary") ? "backup" : "primary" }]);
                            }}
                            className={`rounded-full border px-2.5 py-1 text-xs ${e ? "border-accent bg-accent-soft text-accent" : "border-line text-muted"}`}
                          >
                            {e && <Check className="mr-1 inline size-3" />}
                            {nodeName(n)}
                            {e && mode === "active_standby" && <span className="ml-1 opacity-70">({e.role})</span>}
                          </button>
                        );
                      })}
                    </div>
                  </Field>
                  {mode !== "standalone" && <p className="text-xs text-muted">Failover and DNS records are set in Ingress → Profile → Traffic &amp; DNS.</p>}
                </>
              )}
            </>
          )}

          {step === 3 && (
            <div className="space-y-3 text-[13px]">
              <dl className="grid grid-cols-[140px_1fr] gap-y-1.5">
                <dt className="text-muted">Cluster</dt>
                <dd className="font-medium">{name}</dd>
                <dt className="text-muted">Members</dt>
                <dd>{scope.map(nodeName).join(", ")}</dd>
                <dt className="text-muted">Network</dt>
                <dd className="font-mono">{withNetwork ? `${fabric.subnet} · encrypted mesh · ${fabric.domain}` : "later"}</dd>
                <dt className="text-muted">Ingress</dt>
                <dd>
                  {mode === "skip" ? "Later" : `${MODES.find((m) => m.id === mode)?.title} on ${edges.map((e) => nodeName(nodes.find((n) => n.id === e.server_id)!)).join(", ")}`}
                </dd>
              </dl>
              <div>
                <div className="mb-1 text-xs font-medium text-faint">Forge will</div>
                <ul className="list-disc space-y-0.5 pl-5 text-muted">
                  <li>Create the cluster</li>
                  {plan().map((s) => (
                    <li key={s.label}>{s.label}</li>
                  ))}
                </ul>
              </div>
            </div>
          )}
        </div>
      )}

      <div className="mt-5 flex justify-between">
        <Button variant="ghost" disabled={running} onClick={() => (done ? onOpenChange(false) : step > 0 ? setStep(step - 1) : onOpenChange(false))}>
          {done ? "Close" : step > 0 ? "Back" : "Cancel"}
        </Button>
        {!log.length &&
          (step < STEPS.length - 1 ? (
            <Button variant="primary" disabled={!valid[step]} onClick={() => setStep(step + 1)}>
              Next
            </Button>
          ) : (
            <Button variant="primary" disabled={running || !valid.every(Boolean)} onClick={() => void run()}>
              Create cluster
            </Button>
          ))}
      </div>
    </Modal>
  );
}
