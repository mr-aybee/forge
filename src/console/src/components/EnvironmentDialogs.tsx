/**
 * Dialogs for environments, used by the Project Explorer and the new-service
 * form. An environment always lives in one cluster (D-056) and belongs to one
 * project: creating one needs both, moving one needs a target cluster.
 *
 * Each dialog mounts its form only while open, so the initial state is the
 * reset; callers render them conditionally or pass `undefined` to close.
 */
import { ArrowRight, Loader2, Plus } from "lucide-react";
import { useState } from "react";

import { useClusters, useCreateEnvironment, useDeleteEnvironment, useMoveEnvironment, useProjects, useRenameEnvironment, useSetEnvironmentProject } from "../api/hooks";
import type { Environment } from "../api/types";
import { errorText, useToast } from "../app/toast";
import { InlineError, ProjectForm, conflict } from "./ProjectDialogs";
import { Button, Confirm, Field, Input, Modal, Select } from "./ui";

/** Old → new, as one line (cluster or project moves). */
function FromTo({ from, to }: { from?: string; to?: string }) {
  return (
    <div className="flex min-w-0 items-center gap-2 text-[13px]">
      <span className="truncate text-muted" title={from}>
        {from ?? "Unassigned"}
      </span>
      <ArrowRight className="size-3.5 shrink-0 text-faint" aria-hidden />
      <span className="truncate font-medium" title={to}>
        {to ?? "…"}
      </span>
    </div>
  );
}

function Actions({ onCancel, busy, disabled, label }: { onCancel: () => void; busy: boolean; disabled: boolean; label: string }) {
  return (
    <div className="flex justify-end gap-2">
      <Button type="button" onClick={onCancel}>
        Cancel
      </Button>
      <Button variant="primary" disabled={busy || disabled}>
        {busy && <Loader2 className="size-4 animate-spin" />}
        {label}
      </Button>
    </div>
  );
}

// ------------------------------------------------------------------ new

export interface NewEnvironmentProps {
  onClose: () => void;
  /** Preselects a cluster; with `fixedCluster` it is shown read-only. */
  clusterId?: string;
  fixedCluster?: boolean;
  /** Preselects a project; with `fixedProject` it is shown read-only. */
  projectId?: string;
  fixedProject?: boolean;
  onCreated?: (n: Environment) => void;
}

/** New environment. Offers an inline **New project** when none fits. Render it only while open. */
export function NewEnvironmentModal(props: NewEnvironmentProps) {
  return (
    <Modal open onOpenChange={(o) => !o && props.onClose()} title="New environment" description="A stage of one project, such as test, staging or prod. It lives in one cluster.">
      <NewEnvironmentForm {...props} />
    </Modal>
  );
}

function NewEnvironmentForm({ onClose, clusterId, fixedCluster, projectId, fixedProject, onCreated }: NewEnvironmentProps) {
  const clusters = useClusters().data ?? [];
  const projects = useProjects().data ?? [];
  const create = useCreateEnvironment();
  const toast = useToast();
  const [form, setForm] = useState({ name: "", cluster: clusterId ?? "", project: projectId ?? "", protected: false, newProject: false, err: "" });
  const set = (patch: Partial<typeof form>) => setForm((f) => ({ ...f, err: "", ...patch }));
  const target = clusters.find((c) => c.id === form.cluster);
  const project = projects.find((g) => g.id === form.project);

  if (form.newProject) {
    return (
      <div className="space-y-3">
        <p className="text-xs text-muted">First, the project this environment belongs to.</p>
        <ProjectForm submitLabel="Create project" onCancel={() => set({ newProject: false })} onDone={(g) => set({ project: g.id, newProject: false })} />
      </div>
    );
  }
  return (
    <form
      className="space-y-3"
      onSubmit={(ev) => {
        ev.preventDefault();
        ev.stopPropagation();
        if (!form.project) return set({ err: "Choose a project." });
        if (!form.cluster) return set({ err: "Choose a cluster." });
        create.mutate(
          { name: form.name.trim(), cluster_id: form.cluster, project_id: form.project, protected: form.protected },
          {
            onSuccess: (n) => {
              toast(`Environment “${n?.name ?? form.name}” created in ${project?.name ?? "the project"}`);
              onClose();
              if (n && typeof n === "object" && typeof n.id === "string") onCreated?.(n);
            },
            onError: (e) => set({ err: conflict(e) ? "Name already used in this cluster." : errorText(e) }),
          },
        );
      }}
    >
      <Field label="Project" hint={fixedProject ? undefined : "The product this environment is a stage of."}>
        {fixedProject && project ? (
          <div className="text-[13px] font-medium">{project.name}</div>
        ) : (
          <div className="flex items-center gap-2">
            <div className="min-w-0 flex-1">
              <Select required aria-label="Project" value={form.project} onChange={(e) => set({ project: e.target.value })}>
                <option value="">{projects.length ? "Select a project" : "No projects yet"}</option>
                {projects.map((g) => (
                  <option key={g.id} value={g.id}>
                    {g.name}
                  </option>
                ))}
              </Select>
            </div>
            <Button type="button" onClick={() => set({ newProject: true })} title="Create a project for this environment">
              <Plus className="size-4" /> New project
            </Button>
          </div>
        )}
      </Field>
      <Field label="Name" hint="Unique within its cluster, e.g. test, staging, prod.">
        <Input required autoFocus maxLength={100} value={form.name} onChange={(e) => set({ name: e.target.value })} placeholder="staging" spellCheck={false} />
      </Field>
      <Field label="Cluster">
        {fixedCluster ? (
          <div className="text-[13px] font-medium">{target?.name ?? "this cluster"}</div>
        ) : (
          <Select required aria-label="Cluster" value={form.cluster} onChange={(e) => set({ cluster: e.target.value })}>
            <option value="">{clusters.length ? "Select a cluster" : "No clusters yet"}</option>
            {clusters.map((c) => (
              <option key={c.id} value={c.id}>
                {c.name}
              </option>
            ))}
          </Select>
        )}
      </Field>
      <label className="flex items-start gap-2">
        <input type="checkbox" className="mt-0.5" checked={form.protected} onChange={(e) => set({ protected: e.target.checked })} />
        <span>
          <span className="text-[13px] font-medium">Protected</span>
          <span className="block text-xs text-muted">For production: stricter guardrails on changes.</span>
        </span>
      </label>
      <InlineError text={form.err} />
      <Actions onCancel={onClose} busy={create.isPending} disabled={!form.name.trim() || !form.cluster || !form.project} label="Create" />
    </form>
  );
}

// ------------------------------------------------------------------ move to cluster

/** Move an environment, with its services, into another cluster. Render it only while open. */
export function MoveEnvironmentModal({ env, onClose }: { env: Environment; onClose: () => void }) {
  return (
    <Modal open onOpenChange={(o) => !o && onClose()} title={`Move “${env.name}” to a cluster`}>
      <MoveForm env={env} onClose={onClose} />
    </Modal>
  );
}

function MoveForm({ env, onClose }: { env: Environment; onClose: () => void }) {
  const clusters = useClusters().data ?? [];
  const move = useMoveEnvironment();
  const toast = useToast();
  const [form, setForm] = useState({ to: "", err: "" });
  const from = clusters.find((c) => c.id === env.cluster_id);
  const target = clusters.find((c) => c.id === form.to);
  const options = clusters.filter((c) => c.id !== env.cluster_id);
  return (
    <form
      className="space-y-3"
      onSubmit={(ev) => {
        ev.preventDefault();
        if (!form.to) return;
        move.mutate(
          { id: env.id, cluster_id: form.to },
          {
            onSuccess: (n) => {
              toast(`${n?.name ?? env.name} moved to ${target?.name ?? "the cluster"}`);
              onClose();
            },
            onError: (e) => setForm((f) => ({ ...f, err: conflict(e) ? `${target?.name ?? "That cluster"} already has an environment named “${env.name}”.` : errorText(e) })),
          },
        );
      }}
    >
      <FromTo from={from?.name} to={target?.name} />
      <Field label="Target cluster">
        <Select required aria-label="Target cluster" value={form.to} onChange={(e) => setForm({ to: e.target.value, err: "" })}>
          <option value="">Select a cluster</option>
          {options.map((c) => (
            <option key={c.id} value={c.id}>
              {c.name}
            </option>
          ))}
        </Select>
      </Field>
      {options.length === 0 && <p className="text-xs text-muted">There is no other cluster to move it to. Create a cluster first.</p>}
      <div className="rounded-md border border-line px-3 py-2 text-xs text-muted">
        <div className="mb-1 font-medium text-fg">What happens</div>
        <ul className="list-disc space-y-0.5 pl-4">
          <li>The environment and every service in it belong to {target?.name ?? "the target cluster"} from now on.</li>
          <li>Access does not change: roles granted on the environment, its project or the workspace still apply.</li>
          <li>Nothing is redeployed: running service instances are not restarted.</li>
          <li>Fails if {target?.name ?? "the target cluster"} already has an environment with this name.</li>
        </ul>
      </div>
      <InlineError text={form.err} />
      <Actions onCancel={onClose} busy={move.isPending} disabled={!form.to} label="Move" />
    </form>
  );
}

// ------------------------------------------------------------------ rename

/** Rename an environment. The cluster, project and services stay as they are. Render it only while open. */
export function RenameEnvironmentModal({ env, onClose }: { env: Environment; onClose: () => void }) {
  return (
    <Modal open onOpenChange={(o) => !o && onClose()} title={`Rename “${env.name}”`} description="Only the label changes. Services keep their ids and stay where they are.">
      <RenameForm env={env} onClose={onClose} />
    </Modal>
  );
}

function RenameForm({ env, onClose }: { env: Environment; onClose: () => void }) {
  const rename = useRenameEnvironment();
  const toast = useToast();
  const [form, setForm] = useState({ name: env.name, err: "" });
  const name = form.name.trim();
  return (
    <form
      className="space-y-3"
      onSubmit={(ev) => {
        ev.preventDefault();
        rename.mutate(
          { id: env.id, name },
          {
            onSuccess: () => {
              toast(`Renamed to “${name}”`);
              onClose();
            },
            onError: (e) => setForm((f) => ({ ...f, err: conflict(e) ? "Name already used in this cluster." : errorText(e) })),
          },
        );
      }}
    >
      <Field label="Name">
        <Input required autoFocus maxLength={100} value={form.name} onChange={(e) => setForm({ name: e.target.value, err: "" })} spellCheck={false} />
      </Field>
      <InlineError text={form.err} />
      <Actions onCancel={onClose} busy={rename.isPending} disabled={!name || name === env.name} label="Rename" />
    </form>
  );
}

// ------------------------------------------------------------------ move to project

/** Attach an environment to a project, or move it to another one. Render it only while open. */
export function SetProjectModal({ env, onClose }: { env: Environment; onClose: () => void }) {
  return (
    <Modal open onOpenChange={(o) => !o && onClose()} title={env.project_id ? `Move “${env.name}” to another project` : `Attach “${env.name}” to a project`}>
      <SetProjectForm env={env} onClose={onClose} />
    </Modal>
  );
}

function SetProjectForm({ env, onClose }: { env: Environment; onClose: () => void }) {
  const projects = useProjects().data ?? [];
  const setProject = useSetEnvironmentProject();
  const toast = useToast();
  const [form, setForm] = useState({ to: "", newProject: false, err: "" });
  const from = projects.find((g) => g.id === env.project_id);
  const target = projects.find((g) => g.id === form.to);
  const options = projects.filter((g) => g.id !== env.project_id);
  if (form.newProject) {
    return <ProjectForm submitLabel="Create project" onCancel={() => setForm((f) => ({ ...f, newProject: false }))} onDone={(g) => setForm({ to: g.id, newProject: false, err: "" })} />;
  }
  return (
    <form
      className="space-y-3"
      onSubmit={(ev) => {
        ev.preventDefault();
        if (!form.to) return;
        setProject.mutate(
          { id: env.id, project_id: form.to },
          {
            onSuccess: () => {
              toast(`${env.name} is now part of ${target?.name ?? "the project"}`);
              onClose();
            },
            onError: (e) => setForm((f) => ({ ...f, err: errorText(e) })),
          },
        );
      }}
    >
      <FromTo from={from?.name} to={target?.name} />
      <Field label="Project">
        <div className="flex items-center gap-2">
          <div className="min-w-0 flex-1">
            <Select required aria-label="Project" value={form.to} onChange={(e) => setForm({ to: e.target.value, newProject: false, err: "" })}>
              <option value="">{options.length ? "Select a project" : "No other project"}</option>
              {options.map((g) => (
                <option key={g.id} value={g.id}>
                  {g.name}
                </option>
              ))}
            </Select>
          </div>
          <Button type="button" onClick={() => setForm((f) => ({ ...f, newProject: true }))}>
            <Plus className="size-4" /> New project
          </Button>
        </div>
      </Field>
      <p className="text-xs text-muted">Only the grouping changes: the environment stays in its cluster and nothing is redeployed. Roles granted on the previous project no longer cover it.</p>
      <InlineError text={form.err} />
      <Actions onCancel={onClose} busy={setProject.isPending} disabled={!form.to} label={from ? "Move" : "Attach"} />
    </form>
  );
}

// ------------------------------------------------------------------ delete

/** Delete an empty environment; refused (409) while it still has services or stacks. Render it only while open. */
export function DeleteEnvironmentConfirm({ env, serviceCount, onClose, onDeleted }: { env: Environment; serviceCount: number; onClose: () => void; onDeleted?: () => void }) {
  const del = useDeleteEnvironment();
  const toast = useToast();
  const [err, setErr] = useState("");
  const n = serviceCount;
  return (
    <Confirm
      open
      onOpenChange={(o) => !o && onClose()}
      title={`Delete environment “${env.name}”?`}
      confirmLabel="Delete"
      busy={del.isPending}
      disabled={n > 0}
      onConfirm={() =>
        del.mutate(env.id, {
          onSuccess: () => {
            toast(`Environment “${env.name}” deleted`);
            onClose();
            onDeleted?.();
          },
          onError: (e) => setErr(conflict(e) ? "It still has services or stacks. Remove them first." : errorText(e)),
        })
      }
      body={
        <div className="space-y-2 text-[13px]">
          {n > 0 ? (
            <p>
              It still has {n} service{n === 1 ? "" : "s"}. Delete or move them first; an environment can only be deleted when it is empty.
            </p>
          ) : (
            <p>Removes the environment from its cluster and project. It has no services, so nothing stops running. Role bindings scoped to it stop applying.</p>
          )}
          <InlineError text={err} />
        </div>
      }
    />
  );
}
