// Releases tab of a service: current release, copies (slots), deployments and releases.
import { Check, FileCode2, Loader2, Pause, Play, RotateCcw, Square, ThumbsDown, ThumbsUp, Undo2 } from "lucide-react";
import { useState } from "react";

import {
  artifactLabel,
  deploymentActive,
  useAppDeployments,
  useApplyYaml,
  useControlDeployment,
  useDeployment,
  usePlanYaml,
  useRedeployRelease,
  useRelease,
  useReleases,
  useRollback,
  useSlots,
  type DeploymentControl,
  type PlanView,
} from "../../api/deployments";
import { useServers } from "../../api/hooks";
import type { Service } from "../../api/types";
import { errorText, useToast } from "../../app/toast";
import { Badge, Button, Confirm, CopyButton, Drawer, Empty, ErrorNote, Modal, Skeleton, Status, Table, Td, TextArea, ago, time } from "../ui";

const Num = ({ children }: { children: React.ReactNode }) => <span className="font-mono text-[11px] tabular-nums">{children}</span>;

function Section({ title, actions, children }: { title: string; actions?: React.ReactNode; children: React.ReactNode }) {
  return (
    <section className="mb-5">
      <div className="mb-1.5 flex min-h-7 items-center justify-between gap-2">
        <h3 className="text-xs font-semibold tracking-wide text-muted uppercase">{title}</h3>
        <div className="flex gap-1.5">{actions}</div>
      </div>
      <div className="overflow-hidden rounded-md border border-line">{children}</div>
    </section>
  );
}

export function ReleasesTab({ app, swarm = false }: { app: Service; swarm?: boolean }) {
  const toast = useToast();
  const releases = useReleases(app.id);
  const deployments = useAppDeployments(app.id);
  const slots = useSlots(app.id);
  const nodes = useServers().data ?? [];
  const hostOf = (id: string) => nodes.find((n) => n.id === id)?.hostname ?? id.slice(0, 8);
  const rollback = useRollback();
  const redeploy = useRedeployRelease();
  const [openDep, setOpenDep] = useState<string>();
  const [openRel, setOpenRel] = useState<number>();
  const [applyOpen, setApplyOpen] = useState(false);
  const [confirmRollback, setConfirmRollback] = useState(false);
  const [rollout, setRollout] = useState<number>();

  const current = slots.data?.current_release || 0;
  const list = releases.data ?? [];
  const hasPrevious = list.some((r) => r.number < current);

  const doRollback = () =>
    rollback.mutate(
      { appId: app.id },
      {
        onSuccess: (r) => {
          setConfirmRollback(false);
          toast(`Rolling back to release ${r.deployment.release_number || "—"}`);
          if (r.deployment.id) setOpenDep(r.deployment.id);
        },
        onError: (e) => toast(errorText(e), "error"),
      },
    );
  const doRollout = (n: number) =>
    redeploy.mutate(
      { appId: app.id, release: n },
      {
        onSuccess: (r) => {
          setRollout(undefined);
          toast(`Rolling out release ${n}`);
          if (r.deployment.id) setOpenDep(r.deployment.id);
        },
        onError: (e) => toast(errorText(e), "error"),
      },
    );

  return (
    <div>
      <div className="mb-4 flex flex-wrap items-center gap-3 text-[13px]">
        <span className="text-muted">Current release</span>
        <span className="font-mono text-sm font-semibold tabular-nums">{current ? `#${current}` : "—"}</span>
        <div className="ml-auto flex gap-1.5">
          <Button variant="ghost" onClick={() => setApplyOpen(true)}>
            <FileCode2 className="size-4" /> Apply YAML
          </Button>
          <Button
            disabled={!hasPrevious || rollback.isPending}
            title={hasPrevious ? "Roll back to the release before the current one" : "There is no earlier release to roll back to"}
            onClick={() => setConfirmRollback(true)}
          >
            <Undo2 className="size-4" /> Roll back
          </Button>
        </div>
      </div>

      <Section title="Copies">
        {slots.isPending ? (
          <Skeleton rows={2} />
        ) : slots.isError ? (
          <ErrorNote error={slots.error} />
        ) : !slots.data?.slots.length ? (
          <p className="px-4 py-3 text-xs text-muted">
            {swarm ? "Docker Swarm places this service's tasks; see Overview → Swarm for where they run." : "No running copies yet. Apply a release to start one."}
          </p>
        ) : (
          <Table head={["Node", "Slot", "Release", "Traffic", "State", "Ports"]}>
            {slots.data.slots.map((s) => (
              <tr key={`${s.server_id}/${s.slot}`}>
                <Td className="max-w-48 truncate" title={s.server_id}>
                  {hostOf(s.server_id)}
                </Td>
                <Td>
                  <Num>{s.slot || "—"}</Num>
                </Td>
                <Td>
                  <Num>{s.release_number ? `#${s.release_number}` : "—"}</Num>
                </Td>
                <Td>
                  <Num>{s.weight}%</Num>
                </Td>
                <Td>
                  <Status value={s.state} />
                </Td>
                <Td>
                  <Num>
                    {Object.entries(s.ports)
                      .map(([k, v]) => `${k}→${v}`)
                      .join(", ") || "—"}
                  </Num>
                </Td>
              </tr>
            ))}
          </Table>
        )}
      </Section>

      <Section title="Deployments">
        {deployments.isPending ? (
          <Skeleton rows={3} />
        ) : deployments.isError ? (
          <ErrorNote error={deployments.error} />
        ) : !deployments.data?.length ? (
          <p className="px-4 py-3 text-xs text-muted">No deployments yet.</p>
        ) : (
          <Table head={["Release", "Kind", "Strategy", "State", "By", "Started"]}>
            {deployments.data.map((d) => (
              <tr key={d.id} className="cursor-pointer hover:bg-panel-2" onClick={() => setOpenDep(d.id)}>
                <Td>
                  <button type="button" className="font-mono text-[11px] text-accent tabular-nums hover:underline" onClick={() => setOpenDep(d.id)}>
                    #{d.release_number}
                    {d.from_release ? <span className="text-muted"> ← #{d.from_release}</span> : null}
                  </button>
                </Td>
                <Td className="text-xs">{d.kind.replace("_", " ")}</Td>
                <Td className="text-xs">{d.strategy || "—"}</Td>
                <Td>
                  <Status value={d.state} />
                </Td>
                <Td className="max-w-32 truncate text-xs text-muted" title={d.created_by}>
                  {d.created_by || "—"}
                </Td>
                <Td className="text-xs text-muted" title={time(d.created_at)}>
                  {ago(d.created_at)}
                </Td>
              </tr>
            ))}
          </Table>
        )}
      </Section>

      <Section title="Releases">
        {releases.isPending ? (
          <Skeleton rows={3} />
        ) : releases.isError ? (
          <ErrorNote error={releases.error} />
        ) : !list.length ? (
          <Empty icon={<FileCode2 className="size-5" />} title="No releases yet" action={<Button onClick={() => setApplyOpen(true)}>Apply YAML</Button>}>
            A release is created each time a deployment spec is applied.
          </Empty>
        ) : (
          <Table head={["#", "Artifact", "Created", "Description", ""]}>
            {list.map((r) => (
              <tr key={r.id || r.number} className="hover:bg-panel-2">
                <Td>
                  <button type="button" className="font-mono text-[11px] text-accent tabular-nums hover:underline" onClick={() => setOpenRel(r.number)} title="Show the release source">
                    #{r.number}
                  </button>{" "}
                  {r.number === current && <Badge tone="ok">current</Badge>}
                </Td>
                <Td className="max-w-80">
                  <span className="block truncate font-mono text-[11px]" title={artifactLabel(r.artifact)}>
                    {artifactLabel(r.artifact)}
                  </span>
                </Td>
                <Td className="text-xs whitespace-nowrap text-muted" title={time(r.created_at)}>
                  {ago(r.created_at)}
                </Td>
                <Td className="max-w-64">
                  <span className="block truncate text-xs" title={r.description}>
                    {r.description || "—"}
                  </span>
                </Td>
                <Td className="text-right">
                  <Button variant="ghost" disabled={r.number === current || redeploy.isPending} title={r.number === current ? "Already the current release" : `Roll out release ${r.number} again`} onClick={() => setRollout(r.number)}>
                    <RotateCcw className="size-4" /> Roll out again
                  </Button>
                </Td>
              </tr>
            ))}
          </Table>
        )}
      </Section>

      {confirmRollback && (
        <Confirm
          open
          onOpenChange={(o) => !o && setConfirmRollback(false)}
          title="Roll back"
          body={`Roll back to the release before the current one? Copies of ${app.name} are replaced following the release's rollout policy.`}
          confirmLabel="Roll back"
          busy={rollback.isPending}
          onConfirm={doRollback}
        />
      )}
      {rollout !== undefined && (
        <Confirm
          open
          onOpenChange={(o) => !o && setRollout(undefined)}
          title={`Roll out release ${rollout}`}
          body={`Deploy release ${rollout} of ${app.name} again? Running copies are replaced following its rollout policy.`}
          confirmLabel="Roll out"
          busy={redeploy.isPending}
          onConfirm={() => doRollout(rollout)}
        />
      )}
      {openRel !== undefined && <ReleaseSource appId={app.id} number={openRel} onClose={() => setOpenRel(undefined)} />}
      {applyOpen && (
        <ApplyYamlDialog
          swarm={swarm}
          onClose={() => setApplyOpen(false)}
          onStarted={(id) => {
            setApplyOpen(false);
            if (id) setOpenDep(id);
          }}
        />
      )}
      {openDep && <DeploymentDrawer id={openDep} hostOf={hostOf} onClose={() => setOpenDep(undefined)} />}
    </div>
  );
}

function ReleaseSource({ appId, number, onClose }: { appId: string; number: number; onClose: () => void }) {
  const r = useRelease(appId, number);
  return (
    <Drawer open wide onOpenChange={(o) => !o && onClose()} title={`Release #${number}`}>
      {r.isPending ? (
        <Skeleton rows={6} />
      ) : r.isError ? (
        <ErrorNote error={r.error} />
      ) : (
        <div className="space-y-3 text-[13px]">
          <dl className="grid grid-cols-[8rem_minmax(0,1fr)] gap-x-3 gap-y-1 text-xs">
            <dt className="text-muted">Artifact</dt>
            <dd className="truncate font-mono text-[11px]" title={artifactLabel(r.data.artifact)}>
              {artifactLabel(r.data.artifact)}
            </dd>
            <dt className="text-muted">Strategy</dt>
            <dd>{r.data.policy?.strategy ?? "—"}</dd>
            <dt className="text-muted">Created</dt>
            <dd title={time(r.data.created_at)}>
              {ago(r.data.created_at)} by {r.data.created_by || "—"}
            </dd>
            <dt className="text-muted">Source SHA-256</dt>
            <dd className="truncate font-mono text-[11px]" title={r.data.source_sha256 ?? ""}>
              {r.data.source_sha256 ?? "—"}
            </dd>
          </dl>
          {r.data.source ? (
            <div>
              <div className="mb-1 flex justify-end">
                <CopyButton text={r.data.source} />
              </div>
              <pre className="max-h-[60vh] overflow-auto rounded-md border border-line bg-panel-2 p-3 font-mono text-[11px] whitespace-pre select-text">{r.data.source}</pre>
            </div>
          ) : (
            <p className="text-xs text-muted">This release has no stored source.</p>
          )}
        </div>
      )}
    </Drawer>
  );
}

function ApplyYamlDialog({ swarm, onClose, onStarted }: { swarm: boolean; onClose: () => void; onStarted: (deploymentId: string) => void }) {
  const toast = useToast();
  const [yaml, setYaml] = useState("");
  const plan = usePlanYaml();
  const apply = useApplyYaml();
  // A plan only counts for the text it was made from.
  const [planned, setPlanned] = useState<{ yaml: string; view: PlanView }>();
  const current = planned && planned.yaml === yaml ? planned.view : undefined;

  const doPlan = () => plan.mutate({ yaml }, { onSuccess: (view) => setPlanned({ yaml, view }) });
  const doApply = () =>
    apply.mutate(
      { yaml },
      {
        onSuccess: (r) => {
          toast(`Deployment of release ${r.deployment.release_number || "—"} started`);
          onStarted(r.deployment.id);
        },
        onError: (e) => toast(errorText(e), "error"),
      },
    );

  return (
    <Modal open onOpenChange={(o) => !o && onClose()} title="Apply YAML" description="Plan a deployment spec, review what happens, then apply it." width={760}>
      <TextArea aria-label="Deployment YAML" rows={14} spellCheck={false} value={yaml} onChange={(e) => setYaml(e.target.value)} placeholder={"apiVersion: forge/v1\nkind: Deployment\nmetadata:\n  name: …\n  environment: …"} />
      {plan.isError && <ErrorNote error={errorText(plan.error)} />}
      {current && <PlanSummary p={current} swarm={swarm} />}
      <div className="mt-4 flex justify-end gap-2">
        <Button onClick={onClose}>Cancel</Button>
        <Button disabled={!yaml.trim() || plan.isPending} onClick={doPlan}>
          {plan.isPending && <Loader2 className="size-4 animate-spin" />} Plan
        </Button>
        <Button variant="primary" disabled={!current || apply.isPending} title={current ? undefined : "Plan the current text first"} onClick={doApply}>
          {apply.isPending && <Loader2 className="size-4 animate-spin" />} Apply
        </Button>
      </div>
    </Modal>
  );
}

function PlanSummary({ p, swarm }: { p: PlanView; swarm: boolean }) {
  return (
    <div className="mt-3 space-y-2 rounded-md border border-line p-3 text-[13px]">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs">
        <span>
          <span className="text-muted">Service</span> {p.service || "—"}
          {p.creates_service && (
            <>
              {" "}
              <Badge tone="accent">new</Badge>
            </>
          )}
        </span>
        <span>
          <span className="text-muted">Environment</span> {p.environment || "—"}
        </span>
        <span>
          <span className="text-muted">Release</span> <Num>#{p.release || "—"}</Num>
        </span>
        <span>
          <span className="text-muted">Strategy</span> {p.strategy || "—"}
        </span>
        <span>
          <span className="text-muted">Downtime</span> {p.downtime ? <Badge tone="warn">yes</Badge> : <Badge tone="ok">no</Badge>}
        </span>
      </div>
      <div className="min-w-0 truncate font-mono text-[11px]" title={p.artifact}>
        {p.artifact || "—"}
      </div>
      <div className="text-xs">
        <span className="text-muted">Nodes</span> {p.nodes.length ? p.nodes.join(", ") : swarm ? <span className="text-muted">placed by Docker Swarm</span> : "—"}
        {p.added_nodes.length > 0 && <span className="text-ok"> · +{p.added_nodes.join(", ")}</span>}
        {p.removed_nodes.length > 0 && <span className="text-bad"> · −{p.removed_nodes.join(", ")}</span>}
      </div>
      <div>
        <div className="mb-1 text-xs font-medium text-muted">What happens</div>
        <ol className="list-decimal space-y-0.5 pl-5 text-xs">
          {p.steps.length === 0 && <li className="list-none text-faint">—</li>}
          {p.steps.map((s, i) => (
            <li key={i}>{s}</li>
          ))}
        </ol>
      </div>
      {p.notes.length > 0 && (
        <ul className="space-y-0.5 rounded bg-warn-soft px-3 py-2 text-xs text-warn">
          {p.notes.map((n, i) => (
            <li key={i}>{n}</li>
          ))}
        </ul>
      )}
    </div>
  );
}

const levelClass = { info: "text-muted", warn: "text-warn", error: "text-bad" } as const;

function DeploymentDrawer({ id, hostOf, onClose }: { id: string; hostOf: (id: string) => string; onClose: () => void }) {
  const toast = useToast();
  const d = useDeployment(id);
  const control = useControlDeployment();
  const [confirm, setConfirm] = useState<"abort" | "reject">();
  const [pressed, setPressed] = useState<DeploymentControl>();

  const run = (action: DeploymentControl) => {
    setPressed(action);
    control.mutate(
      { id, action },
      {
        onSuccess: () => {
          setConfirm(undefined);
          toast(`Deployment ${action === "abort" ? "aborted" : action === "reject" ? "rejected" : action === "approve" ? "approved" : action === "pause" ? "paused" : "resumed"}`);
        },
        onError: (e) => toast(errorText(e), "error"),
      },
    );
  };
  const spin = (a: DeploymentControl) => control.isPending && pressed === a && <Loader2 className="size-4 animate-spin" />;

  const v = d.data;
  const active = deploymentActive(v);
  return (
    <Drawer open wide onOpenChange={(o) => !o && onClose()} title={v?.service ? `Deployment of ${v.service}` : "Deployment"}>
      {d.isPending ? (
        <Skeleton rows={6} />
      ) : d.isError ? (
        <ErrorNote error={d.error} />
      ) : (
        <div className="space-y-4 text-[13px]">
          <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
            <Status value={d.data.state} />
            <span>
              Release <Num>#{d.data.release_number}</Num>
              {d.data.from_release ? (
                <span className="text-muted">
                  {" "}
                  (from <Num>#{d.data.from_release}</Num>)
                </span>
              ) : null}
            </span>
            <span className="text-xs text-muted">{d.data.strategy || "—"}</span>
            <span className="text-xs text-muted">{d.data.kind.replace("_", " ")}</span>
            <span className="text-xs text-muted" title={time(d.data.created_at)}>
              started {ago(d.data.created_at)}
            </span>
            {active && <Loader2 className="size-3.5 animate-spin text-accent" aria-label="Live" />}
          </div>
          {(active && d.data.can_control) || (d.data.state === "awaiting_approval" && d.data.can_approve) ? (
            <div className="flex flex-wrap gap-1.5">
              {d.data.state === "awaiting_approval" && d.data.can_approve && (
                <>
                  <Button variant="primary" disabled={control.isPending} onClick={() => run("approve")}>
                    {spin("approve") || <ThumbsUp className="size-4" />} Approve
                  </Button>
                  <Button variant="danger" disabled={control.isPending} onClick={() => setConfirm("reject")}>
                    <ThumbsDown className="size-4" /> Reject
                  </Button>
                </>
              )}
              {active && d.data.can_control && (
                <>
                  {d.data.state === "paused" ? (
                    <Button disabled={control.isPending} onClick={() => run("resume")}>
                      {spin("resume") || <Play className="size-4" />} Resume
                    </Button>
                  ) : (
                    <Button disabled={control.isPending} onClick={() => run("pause")}>
                      {spin("pause") || <Pause className="size-4" />} Pause
                    </Button>
                  )}
                  <Button variant="danger" disabled={control.isPending} onClick={() => setConfirm("abort")}>
                    <Square className="size-4" /> Abort
                  </Button>
                </>
              )}
            </div>
          ) : null}
          {d.data.error && <p className="rounded-md bg-bad-soft px-3 py-2 text-xs break-words text-bad select-text">{d.data.error}</p>}
          <div>
            <div className="mb-1 text-xs font-medium text-muted">Steps</div>
            {d.data.steps.length ? (
              <ol className="space-y-0.5 text-xs">
                {d.data.steps.map((s, i) => {
                  const done = i < d.data.step;
                  const now = i === d.data.step && active;
                  return (
                    <li key={i} className={`flex items-start gap-2 ${done ? "text-muted" : now ? "text-fg" : "text-faint"}`}>
                      <span className="mt-0.5 grid size-3.5 shrink-0 place-items-center">
                        {done ? <Check className="size-3.5 text-ok" /> : now ? <Loader2 className="size-3.5 animate-spin text-accent" /> : <span className="font-mono text-[10px]">{i + 1}</span>}
                      </span>
                      <span className="min-w-0">{s}</span>
                    </li>
                  );
                })}
              </ol>
            ) : (
              <p className="text-xs text-muted">—</p>
            )}
          </div>
          <div>
            <div className="mb-1 flex items-center justify-between">
              <span className="text-xs font-medium text-muted">Events</span>
              {d.data.events.length > 0 && <CopyButton text={d.data.events.map((e) => `${time(e.at)} ${e.level} ${e.server_id ? hostOf(e.server_id) + " " : ""}${e.message}`).join("\n")} />}
            </div>
            {d.data.events.length ? (
              <div className="max-h-[40vh] overflow-auto rounded-md border border-line bg-panel-2 p-2 font-mono text-[11px] select-text">
                {d.data.events.map((e) => (
                  <div key={e.seq} className="flex gap-2">
                    <span className="shrink-0 text-faint tabular-nums" title={time(e.at)}>
                      {e.at ? new Date(e.at).toLocaleTimeString() : "—"}
                    </span>
                    {e.server_id && (
                      <span className="max-w-32 shrink-0 truncate text-muted" title={e.server_id}>
                        {hostOf(e.server_id)}
                      </span>
                    )}
                    <span className={`min-w-0 break-words ${levelClass[e.level]}`}>{e.message}</span>
                  </div>
                ))}
              </div>
            ) : (
              <p className="text-xs text-muted">No events yet.</p>
            )}
          </div>
        </div>
      )}
      {confirm && (
        <Confirm
          open
          onOpenChange={(o) => !o && setConfirm(undefined)}
          title={confirm === "abort" ? "Abort deployment" : "Reject deployment"}
          body={
            confirm === "abort"
              ? "Stop this deployment now? Steps already done stay done; copies of the new release may be left running beside the old one."
              : "Reject this deployment? It stops at the approval gate and the current release keeps serving."
          }
          confirmLabel={confirm === "abort" ? "Abort" : "Reject"}
          busy={control.isPending}
          onConfirm={() => run(confirm)}
        />
      )}
    </Drawer>
  );
}
