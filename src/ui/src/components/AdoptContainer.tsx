import { useNavigate } from "@tanstack/react-router";
import { Loader2 } from "lucide-react";
import { useState } from "react";

import { ApiError } from "../api/client";
import { useAdoptService, useAllEnvironments } from "../api/hooks";
import { errorText, useToast } from "../app/toast";
import { Button, ErrorNote, Field, Input, Modal, Mono, Select } from "./ui";

/** Containers Forge already runs as services; they cannot be adopted again. */
export const isForgeServiceContainer = (name: string) => name.replace(/^\//, "").startsWith("forge-app-");

/** A service name from a container name: lower-case, `[a-z0-9-]` only, trimmed, at most 40 characters. */
export function serviceNameFrom(container: string): string {
  return container
    .replace(/^\//, "")
    .toLowerCase()
    .replace(/[^a-z0-9-]/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 40)
    .replace(/-+$/, "");
}

function adoptError(e: unknown): string {
  if (e instanceof ApiError && e.code === "agent_update_required") return "The agent on this node is too old to adopt containers. Update the agent first.";
  if (e instanceof ApiError && e.status === 409) return `${errorText(e)} (the name may already exist in this environment, or the container is already adopted).`;
  if (e instanceof ApiError && e.status === 403) return `${errorText(e)} Adopting needs deploy on the environment, and server.manage and container.inspect_env on the node.`;

  return errorText(e);
}

/** "Adopt as service": takes over a running container without restarting it. */
export function AdoptContainerDialog({ serverId, container, onClose }: { serverId: string; container: string; onClose: () => void }) {
  const envs = useAllEnvironments();
  const adopt = useAdoptService();
  const toast = useToast();
  const navigate = useNavigate();
  const [form, setForm] = useState({ environment: "", name: serviceNameFrom(container) });
  const [err, setErr] = useState<string>();
  const list = envs.data ?? [];
  const nameOk = form.name === "" || /^[a-z0-9]([a-z0-9-]{0,38}[a-z0-9])?$/.test(form.name);

  const submit = () => {
    setErr(undefined);
    adopt.mutate(
      { environment_id: form.environment, server_id: serverId, container: container.replace(/^\//, ""), ...(form.name ? { name: form.name } : {}) },
      {
        onSuccess: (r) => {
          toast(`Adopted ${container} as service ${r?.service?.name ?? form.name}`);
          onClose();
          if (r?.service?.id) void navigate({ to: "/services/$serviceId", params: { serviceId: r.service.id } });
        },
        onError: (e) => setErr(adoptError(e)),
      },
    );
  };

  return (
    <Modal open onOpenChange={(o) => !o && onClose()} title="Adopt as service" description={`Container ${container}`}>
      <form
        className="space-y-3 text-[13px]"
        onSubmit={(e) => {
          e.preventDefault();
          submit();
        }}
      >
        <p className="text-xs text-muted">
          Forge reads the container&apos;s image, environment, ports, mounts and restart policy, and does not restart it. The first deploy replaces it with a Forge-managed container.
        </p>
        {envs.error ? (
          <ErrorNote error={envs.error} />
        ) : (
          <Field label="Environment" hint={!envs.isLoading && list.length === 0 ? "No environments you can see. Create one in Projects first." : undefined}>
            <Select value={form.environment} onChange={(e) => setForm({ ...form, environment: e.target.value })} required disabled={envs.isLoading}>
              <option value="">{envs.isLoading ? "Loading…" : "Choose an environment"}</option>
              {list.map((e) => (
                <option key={e.id} value={e.id}>
                  {e.name}
                </option>
              ))}
            </Select>
          </Field>
        )}
        <Field label="Service name" hint={nameOk ? "Optional; lower-case letters, digits and dashes, up to 40." : "Use lower-case letters, digits and dashes (up to 40), starting and ending with a letter or digit."}>
          <Input value={form.name} maxLength={40} onChange={(e) => setForm({ ...form, name: e.target.value })} aria-invalid={!nameOk} placeholder={serviceNameFrom(container)} />
        </Field>
        <p className="text-xs text-faint">
          Needs an MFA-verified sign-in to read the container&apos;s environment; you are asked to verify if needed. Source: <Mono>{container}</Mono>
        </p>
        {err && (
          <p role="alert" className="rounded border border-bad/30 bg-bad-soft px-3 py-2 text-xs text-bad">
            {err}
          </p>
        )}
        <div className="flex justify-end gap-2">
          <Button type="button" onClick={onClose}>
            Cancel
          </Button>
          <Button type="submit" variant="primary" disabled={!form.environment || !nameOk || adopt.isPending}>
            {adopt.isPending && <Loader2 className="size-4 animate-spin" />}
            Adopt
          </Button>
        </div>
      </form>
    </Modal>
  );
}
