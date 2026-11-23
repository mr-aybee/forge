/**
 * Dialogs for projects. A **Project** is a product such as "shop"; it has
 * environments (test, staging, prod…) and the services in them. Each dialog
 * is rendered only while open, so its initial state is the reset.
 */
import { Loader2 } from "lucide-react";
import { useState } from "react";

import { ApiError } from "../api/client";
import { useDeleteProject, useSaveProject } from "../api/hooks";
import type { ProjectBase, Project } from "../api/types";
import { errorText, useToast } from "../app/toast";
import { Button, Confirm, Field, Input, Modal, TextArea } from "./ui";

export const conflict = (e: unknown) => e instanceof ApiError && e.status === 409;

/** Server rule: 1-63 lower-case letters, digits and dashes. */
const NAME = /^[a-z0-9-]{1,63}$/;

export function InlineError({ text }: { text: string }) {
  if (!text) return null;
  return (
    <p role="alert" className="rounded-md bg-bad-soft px-2.5 py-1.5 text-xs text-bad">
      {text}
    </p>
  );
}

/** Name + description form shared by the modal and the inline "New project" in the environment dialogs. */
export function ProjectForm({ initial, onDone, onCancel, submitLabel }: { initial?: Pick<ProjectBase, "id" | "name" | "description">; onDone: (g: ProjectBase) => void; onCancel?: () => void; submitLabel?: string }) {
  const save = useSaveProject();
  const [form, setForm] = useState({ name: initial?.name ?? "", description: initial?.description ?? "", err: "" });
  const { name, description, err } = form;
  const set = (patch: Partial<typeof form>) => setForm((f) => ({ ...f, err: "", ...patch }));
  const setErr = (e: string) => setForm((f) => ({ ...f, err: e }));
  const nameOk = NAME.test(name.trim());
  return (
    <form
      className="space-y-3"
      onSubmit={(ev) => {
        ev.preventDefault();
        ev.stopPropagation();
        if (!nameOk) return setErr("Use 1-63 lower-case letters, digits and dashes.");
        save.mutate(
          { id: initial?.id, name: name.trim(), description: description.trim() },
          {
            onSuccess: (g) => onDone(g && typeof g === "object" ? g : { id: initial?.id ?? "", workspace_id: "", name: name.trim(), description: description.trim(), created_at: 0 }),
            onError: (e) => setErr(conflict(e) ? "A project with this name already exists." : errorText(e)),
          },
        );
      }}
    >
      <Field label="Name" hint={name && !nameOk ? <span className="text-bad">Lower-case letters, digits and dashes, up to 63 characters.</span> : "The product, e.g. shop or billing. Lower-case letters, digits and dashes; unique in the workspace."}>
        <Input
          required
          autoFocus
          maxLength={63}
          pattern="[a-z0-9\-]{1,63}"
          aria-invalid={!!name && !nameOk}
          value={name}
          onChange={(e) => set({ name: e.target.value.toLowerCase() })}
          placeholder="shop"
          spellCheck={false}
        />
      </Field>
      <Field label="Description" hint="Optional: what it is and who owns it.">
        <TextArea rows={2} maxLength={500} value={description} onChange={(e) => set({ description: e.target.value })} placeholder="Customer-facing web shop; owned by the commerce team." />
      </Field>
      <InlineError text={err} />
      <div className="flex justify-end gap-2">
        {onCancel && (
          <Button type="button" onClick={onCancel}>
            Cancel
          </Button>
        )}
        <Button variant="primary" disabled={save.isPending || !nameOk}>
          {save.isPending && <Loader2 className="size-4 animate-spin" />}
          {submitLabel ?? (initial?.id ? "Save" : "Create")}
        </Button>
      </div>
    </form>
  );
}

/** New project, or rename/describe an existing one (`project` set). Render it only while open. */
export function ProjectModal({ onClose, project, onCreated }: { onClose: () => void; project?: Pick<ProjectBase, "id" | "name" | "description">; onCreated?: (g: ProjectBase) => void }) {
  const toast = useToast();
  return (
    <Modal
      open
      onOpenChange={(o) => !o && onClose()}
      title={project ? `Edit “${project.name}”` : "New project"}
      description={project ? undefined : "A product such as shop or billing. Add environments (test, staging, prod…) to it, then services to each environment."}
    >
      <ProjectForm
        initial={project}
        onCancel={onClose}
        onDone={(g) => {
          toast(project ? `Project “${g.name}” saved` : `Project “${g.name}” created`);
          onClose();
          onCreated?.(g);
        }}
      />
    </Modal>
  );
}

/** Delete a project; refused (409) while it still has environments, which the dialog explains up front. Render it only while open. */
export function DeleteProjectConfirm({ project, onClose, onDeleted }: { project: Project; onClose: () => void; onDeleted?: () => void }) {
  const del = useDeleteProject();
  const toast = useToast();
  const [err, setErr] = useState("");
  const envs = project.environments;
  return (
    <Confirm
      open
      onOpenChange={(o) => !o && onClose()}
      title={`Delete project “${project.name}”?`}
      confirmLabel="Delete"
      busy={del.isPending}
      disabled={envs.length > 0}
      onConfirm={() =>
        del.mutate(project.id, {
          onSuccess: () => {
            toast(`Project “${project.name}” deleted`);
            onClose();
            onDeleted?.();
          },
          onError: (e) => setErr(conflict(e) ? "It still has environments. Delete or move them first." : errorText(e)),
        })
      }
      body={
        <div className="space-y-2 text-[13px]">
          {envs.length > 0 ? (
            <p>
              It still has {envs.length} environment{envs.length === 1 ? "" : "s"} ({envs.map((e) => e.name).join(", ")}). Delete or move them first; a project can only be deleted when it is empty.
            </p>
          ) : (
            <p>Removes the project. It has no environments, so no service is affected. Role bindings scoped to it stop applying.</p>
          )}
          <InlineError text={err} />
        </div>
      }
    />
  );
}
