import { Cloud, Container, GitBranch, KeyRound, Package, Loader2, Pencil, Plus, Server, Trash2 } from "lucide-react";
import { type ReactNode, useState } from "react";

import { ApiError } from "../api/client";
import { REGISTRY_KINDS, type Registry, type RegistryBody, type RegistryKind, useCreateRegistry, useDeleteRegistry, useRegistries, useUpdateRegistry } from "../api/registries";
import { errorText, useToast } from "../app/toast";
import { SettingsTabs } from "../components/SettingsTabs";
import { ago, Button, Confirm, CopyIcon, Empty, ErrorNote, Field, Input, Modal, PageHeader, Panel, RowMenu, type RowAction, Select, Skeleton, Table, Td, time, useRowMenu } from "../components/ui";

const KIND: Record<RegistryKind, { label: string; icon: ReactNode; host: string; placeholder: string }> = {
  docker_hub: { label: "Docker Hub", icon: <Container className="size-3.5" />, host: "docker.io", placeholder: "docker.io" },
  ghcr: { label: "GitHub (ghcr.io)", icon: <Package className="size-3.5" />, host: "ghcr.io", placeholder: "ghcr.io" },
  gitlab: { label: "GitLab", icon: <GitBranch className="size-3.5" />, host: "registry.gitlab.com", placeholder: "registry.gitlab.com" },
  ecr: { label: "Amazon ECR", icon: <Cloud className="size-3.5" />, host: "", placeholder: "<account>.dkr.ecr.<region>.amazonaws.com" },
  custom: { label: "Custom", icon: <Server className="size-3.5" />, host: "", placeholder: "registry.example.com:5000" },
};

/** Container registry credentials Forge uses to pull and push images on nodes (D-157). */
export function Registries() {
  const q = useRegistries();
  const del = useDeleteRegistry();
  const toast = useToast();
  const rm = useRowMenu();
  const [dialog, setDialog] = useState<{ kind: "add" } | { kind: "edit"; r: Registry } | { kind: "delete"; r: Registry } | null>(null);
  const close = () => setDialog(null);
  const denied = q.error instanceof ApiError && q.error.status === 403;
  const missing = q.error instanceof ApiError && (q.error.status === 404 || q.error.status === 405);
  const rows = q.data ?? [];

  return (
    <>
      <PageHeader
        eyebrow="Administration"
        title="Settings"
        description="Settings of this Forge server. Your own account and preferences are under Profile."
      />
      <SettingsTabs />
      <Panel
        title="Container registries"
        description="Credentials nodes use to pull private images and push tagged ones. Each host appears once."
        actions={
          <Button variant="primary" onClick={() => setDialog({ kind: "add" })} disabled={denied || missing}>
            <Plus className="size-4" /> Add registry
          </Button>
        }
      >
        {denied ? (
          <p className="p-4 text-[13px] text-muted">You cannot see the registries of this workspace.</p>
        ) : missing ? (
          <ErrorNote error="This Forge server does not offer registries yet. Update Forge to use them." />
        ) : q.error ? (
          <ErrorNote error={q.error} />
        ) : q.isLoading ? (
          <Skeleton rows={3} />
        ) : rows.length === 0 ? (
          <Empty
            icon={<KeyRound className="size-5" />}
            title="No registries"
            action={
              <Button variant="primary" onClick={() => setDialog({ kind: "add" })}>
                <Plus className="size-4" /> Add registry
              </Button>
            }
          >
            Add one to pull private images and to push images from a node's Docker tab.
          </Empty>
        ) : (
          <Table head={["Name", "Kind", "Host", "User", "Updated", ""]}>
            {rows.map((r) => {
              const k = KIND[r.kind];
              const actions: RowAction[] = [
                { label: "Edit…", icon: Pencil, tone: "accent", onSelect: () => setDialog({ kind: "edit", r }) },
                { label: "Delete…", icon: Trash2, danger: true, onSelect: () => setDialog({ kind: "delete", r }) },
              ];
              return (
                <tr key={r.id} className="hover:bg-panel-2/60" {...rm.rowProps(actions, `Actions for ${r.name}`)}>
                  <Td className="max-w-[220px]">
                    <span className="block truncate text-[13px] font-medium" title={r.name}>
                      {r.name || "—"}
                    </span>
                  </Td>
                  <Td className="text-xs whitespace-nowrap text-muted">
                    <span className="inline-flex items-center gap-1.5">
                      {k.icon}
                      {k.label}
                    </span>
                  </Td>
                  <Td className="max-w-[300px]">
                    <div className="flex min-w-0 items-center gap-1.5">
                      <span className="truncate font-mono text-[12px] select-all" title={r.host}>
                        {r.host || "—"}
                      </span>
                      {r.host && <CopyIcon text={r.host} label="Copy host" />}
                    </div>
                  </Td>
                  <Td className="max-w-[180px] truncate font-mono text-[11px] text-muted" title={r.username}>
                    {r.username || "—"}
                  </Td>
                  <Td className="text-xs whitespace-nowrap text-muted" title={r.updated_at ? time(r.updated_at) : undefined}>
                    {r.updated_at ? ago(r.updated_at) : "—"}
                  </Td>
                  <Td className="text-right">
                    <RowMenu actions={actions} label={`Actions for ${r.name}`} />
                  </Td>
                </tr>
              );
            })}
          </Table>
        )}
        {rm.menu}
      </Panel>

      {dialog?.kind === "add" && <RegistryDialog onClose={close} />}
      {dialog?.kind === "edit" && <RegistryDialog existing={dialog.r} onClose={close} />}
      {dialog?.kind === "delete" && (
        <Confirm
          open
          onOpenChange={(o) => !o && close()}
          title={`Delete ${dialog.r.name}?`}
          body={
            <>
              Forge forgets the credentials for <span className="font-mono text-fg">{dialog.r.host}</span>. Pulls of private images from it and pushes to it fail
              until you add it again. Images already on nodes and running containers are not affected.
            </>
          }
          confirmLabel="Delete"
          busy={del.isPending}
          onConfirm={() =>
            del.mutate(dialog.r.id, {
              onSuccess: () => {
                toast(`Deleted ${dialog.r.name}`);
                close();
              },
              onError: (e) => toast(errorText(e), "error"),
            })
          }
        />
      )}
    </>
  );
}

function RegistryDialog({ existing, onClose }: { existing?: Registry; onClose: () => void }) {
  const create = useCreateRegistry();
  const update = useUpdateRegistry();
  const toast = useToast();
  const [form, setForm] = useState<Required<RegistryBody>>({
    name: existing?.name ?? "",
    kind: existing?.kind ?? "docker_hub",
    host: existing?.host ?? KIND.docker_hub.host,
    username: existing?.username ?? "",
    secret: "",
  });
  const [error, setError] = useState<unknown>();
  const busy = create.isPending || update.isPending;
  const locked = form.kind === "docker_hub";
  const set = (p: Partial<RegistryBody>) => setForm((f) => ({ ...f, ...p }));
  const valid = form.name.trim() && form.host.trim() && form.username.trim() && (existing || form.secret);

  const submit = () => {
    setError(undefined);
    const body: RegistryBody = { name: form.name.trim(), kind: form.kind, host: form.host.trim(), username: form.username.trim() };
    if (form.secret) body.secret = form.secret;
    const done = { onSuccess: () => { toast(existing ? `Saved ${body.name}` : `Added ${body.name}`); onClose(); }, onError: (e: unknown) => setError(e) };
    if (existing) update.mutate({ id: existing.id, body }, done);
    else create.mutate(body, done);
  };

  const errMsg =
    error instanceof ApiError && error.status === 409
      ? `A registry for ${form.host.trim()} already exists. Edit that one instead.`
      : error instanceof ApiError && error.status === 422
        ? errorText(error) || "Docker Hub's host must be docker.io."
        : error
          ? errorText(error)
          : undefined;

  return (
    <Modal open onOpenChange={(o) => !o && onClose()} title={existing ? `Edit ${existing.name}` : "Add registry"} description="Nodes log in with these credentials to pull and push images.">
      <form
        className="space-y-3"
        onSubmit={(e) => {
          e.preventDefault();
          if (valid) submit();
        }}
      >
        <Field label="Name">
          <Input value={form.name} onChange={(e) => set({ name: e.target.value })} placeholder="e.g. Company GHCR" maxLength={80} required autoFocus />
        </Field>
        <Field label="Kind">
          <Select value={form.kind} onChange={(e) => { const kind = e.target.value as RegistryKind; set({ kind, host: KIND[kind].host }); }}>
            {REGISTRY_KINDS.map((k) => (
              <option key={k} value={k}>
                {KIND[k].label}
              </option>
            ))}
          </Select>
        </Field>
        <Field label="Host" hint={locked ? "Docker Hub always uses docker.io." : "Registry host name, with a port if it is not 443. No scheme or path."}>
          <Input className="font-mono" value={form.host} onChange={(e) => set({ host: e.target.value })} placeholder={KIND[form.kind].placeholder} disabled={locked} required spellCheck={false} />
        </Field>
        <Field label="Username" hint={form.kind === "ecr" ? "Use AWS for ECR." : undefined}>
          <Input className="font-mono" value={form.username} onChange={(e) => set({ username: e.target.value })} autoComplete="off" spellCheck={false} required />
        </Field>
        <Field
          label="Password or token"
          hint={
            <>
              {existing && <span className="block">Leave empty to keep the stored one.</span>}
              Use a token limited to the repositories and scopes nodes need (read, plus write to push).
              {form.kind === "ecr" && " ECR tokens expire after 12 hours; update the token before then."}
            </>
          }
        >
          <Input type="password" value={form.secret} onChange={(e) => set({ secret: e.target.value })} autoComplete="new-password" placeholder={existing ? "Leave empty to keep" : undefined} required={!existing} />
        </Field>
        {errMsg && <ErrorNote error={errMsg} />}
        <div className="flex justify-end gap-2 pt-1">
          <Button type="button" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" disabled={busy || !valid}>
            {busy && <Loader2 className="size-4 animate-spin" />}
            {existing ? "Save" : "Add registry"}
          </Button>
        </div>
      </form>
    </Modal>
  );
}
