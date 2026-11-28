// Create / duplicate a role, create / rename a group. Each dialog mounts when
// open, so its initial state is the reset. These write immediately (they
// create things; nobody gains access until an assignment is applied).

import { Loader2 } from "lucide-react";
import { useState } from "react";

import { useAccessCatalog, useCreateGroup, useCreateProfile, useDuplicateProfile, useUpdateGroup, type Group, type Profile } from "../../api/accessControl";
import { errorText, useToast } from "../../app/toast";
import { Button, ErrorNote, Field, Input, Modal, Skeleton } from "../ui";
import { PermissionPicker } from "./PermissionPicker";

export function CreateRoleDialog({ onClose, onCreated }: { onClose: () => void; onCreated: (p: Profile) => void }) {
  const catalog = useAccessCatalog();
  const create = useCreateProfile();
  const toast = useToast();
  const [f, setF] = useState({ name: "", description: "", actions: new Set<string>() });
  const submit = () =>
    create.mutate(
      { name: f.name.trim(), description: f.description.trim(), actions: [...f.actions].sort() },
      {
        onSuccess: (p) => {
          toast(`Role ${p.name} created`);
          onCreated(p);
        },
        onError: (e) => toast(errorText(e), "error"),
      },
    );
  return (
    <Modal open onOpenChange={(o) => !o && !create.isPending && onClose()} title="New role" description="A named set of permissions. Assign it to users or groups at a scope to grant it." wide>
      <form
        className="space-y-3"
        onSubmit={(e) => {
          e.preventDefault();
          submit();
        }}
      >
        <div className="grid gap-3 sm:grid-cols-2">
          <Field label="Name">
            <Input value={f.name} onChange={(e) => setF({ ...f, name: e.target.value })} placeholder="Release manager" required maxLength={80} autoFocus />
          </Field>
          <Field label="Description">
            <Input value={f.description} onChange={(e) => setF({ ...f, description: e.target.value })} placeholder="Deploys and rolls back services" maxLength={300} />
          </Field>
        </div>
        {catalog.isPending ? <Skeleton rows={4} /> : catalog.isError ? <ErrorNote error={catalog.error} /> : <PermissionPicker catalog={catalog.data.actions} selected={f.actions} onChange={(actions) => setF({ ...f, actions })} />}
        <div className="flex justify-end gap-2">
          <Button type="button" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" disabled={create.isPending || !f.name.trim() || f.actions.size === 0} title={f.actions.size === 0 ? "Pick at least one permission" : undefined}>
            {create.isPending && <Loader2 className="size-4 animate-spin" />} Create role
          </Button>
        </div>
      </form>
    </Modal>
  );
}

export function DuplicateRoleDialog({ role, onClose, onCreated }: { role: Profile; onClose: () => void; onCreated: (p: Profile) => void }) {
  const dup = useDuplicateProfile();
  const toast = useToast();
  const [name, setName] = useState(`${role.name} (copy)`);
  return (
    <Modal open onOpenChange={(o) => !o && !dup.isPending && onClose()} title={`Duplicate ${role.name}`} description="The copy is a custom role you can edit; the original stays as it is.">
      <form
        className="space-y-3"
        onSubmit={(e) => {
          e.preventDefault();
          dup.mutate(
            { id: role.id, name: name.trim() },
            {
              onSuccess: (p) => {
                toast(`Role ${p.name} created`);
                onCreated(p);
              },
              onError: (e) => toast(errorText(e), "error"),
            },
          );
        }}
      >
        <Field label="Name">
          <Input value={name} onChange={(e) => setName(e.target.value)} required maxLength={80} autoFocus />
        </Field>
        <div className="flex justify-end gap-2">
          <Button type="button" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" disabled={dup.isPending || !name.trim()}>
            {dup.isPending && <Loader2 className="size-4 animate-spin" />} Duplicate
          </Button>
        </div>
      </form>
    </Modal>
  );
}

/** New group, or rename / redescribe `group`. */
export function GroupDialog({ group, onClose, onSaved }: { group?: Group; onClose: () => void; onSaved: (g: Group) => void }) {
  const create = useCreateGroup();
  const update = useUpdateGroup();
  const toast = useToast();
  const [f, setF] = useState({ name: group?.name ?? "", description: group?.description ?? "" });
  const busy = create.isPending || update.isPending;
  const done = {
    onSuccess: (g: Group) => {
      toast(group ? `Group ${g.name} saved` : `Group ${g.name} created`);
      onSaved(g);
    },
    onError: (e: unknown) => toast(errorText(e), "error"),
  };
  return (
    <Modal open onOpenChange={(o) => !o && !busy && onClose()} title={group ? `Edit ${group.name}` : "New group"} description={group ? undefined : "Members of a group get every role assigned to the group."}>
      <form
        className="space-y-3"
        onSubmit={(e) => {
          e.preventDefault();
          const b = { name: f.name.trim(), description: f.description.trim() };
          if (group) update.mutate({ id: group.id, ...b }, done);
          else create.mutate(b, done);
        }}
      >
        <Field label="Name">
          <Input value={f.name} onChange={(e) => setF({ ...f, name: e.target.value })} placeholder="platform-oncall" required maxLength={80} autoFocus />
        </Field>
        <Field label="Description">
          <Input value={f.description} onChange={(e) => setF({ ...f, description: e.target.value })} maxLength={300} />
        </Field>
        <div className="flex justify-end gap-2">
          <Button type="button" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" disabled={busy || !f.name.trim()}>
            {busy && <Loader2 className="size-4 animate-spin" />} {group ? "Save" : "Create group"}
          </Button>
        </div>
      </form>
    </Modal>
  );
}
