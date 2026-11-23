// The environment an object store belongs to (D-107): the picker used when a
// store is installed, connected or adopted, the label and link shown for a
// store, the dialog that changes it, and the list of stores an environment
// owns. A store without an environment is shared.
import { Link } from "@tanstack/react-router";
import { Database, Loader2 } from "lucide-react";
import { useState } from "react";

import { useAllEnvironments, useProjects } from "../../api/hooks";
import { ApiError } from "../../api/client";
import { ATTACHED_ELSEWHERE, useObjectStores, useSetStoreOwner } from "../../api/storage";
import type { ObjectStore } from "../../api/types";
import { errorText, useToast } from "../../app/toast";
import { Button, Confirm, ErrorNote, Field, Modal, Select, Status } from "../ui";

/** The picker's choice for a store that belongs to no environment. */
export const SHARED_OPTION = "Shared (no environment)";

interface EnvironmentOption {
  id: string;
  name: string;
  /** The project it belongs to; undefined for an unassigned environment. */
  app?: { id: string; name: string };
}

/** The environments the caller can see, each with its project. */
function useEnvironments() {
  const environments = useAllEnvironments();
  const groups = useProjects();
  const apps = new Map((groups.data ?? []).map((g) => [g.id, g.name]));
  const options: EnvironmentOption[] = (environments.data ?? [])
    .map((n) => {
      const appName = n.project_id ? apps.get(n.project_id) : undefined;
      return { id: n.id, name: n.name, app: n.project_id && appName ? { id: n.project_id, name: appName } : undefined };
    })
    .sort((a, b) => (a.app?.name ?? "").localeCompare(b.app?.name ?? "") || a.name.localeCompare(b.name));
  return { options, isLoading: environments.isLoading || groups.isLoading, error: environments.error };
}

const UNASSIGNED = "No project";

/**
 * Optional **Environment** of a store. `""` is a shared store. The hint
 * says what the choice gives and what it needs.
 */
export function EnvironmentField({ value, onChange }: { value: string; onChange: (environmentId: string) => void }) {
  const envs = useEnvironments();
  const byApp = new Map<string, EnvironmentOption[]>();
  for (const o of envs.options) byApp.set(o.app?.name ?? UNASSIGNED, [...(byApp.get(o.app?.name ?? UNASSIGNED) ?? []), o]);
  return (
    <Field
      label="Environment"
      hint={
        envs.error ? (
          <span className="text-bad">Couldn't list the environments: {errorText(envs.error)}</span>
        ) : (
          "Optional. People with a role on the environment can browse the store and attach its buckets to the environment's services. Needs the deploy permission there."
        )
      }
    >
      <Select aria-label="Environment" value={value} disabled={envs.isLoading} onChange={(e) => onChange(e.target.value)}>
        <option value="">{envs.isLoading ? "Loading…" : SHARED_OPTION}</option>
        {[...byApp.entries()].map(([app, list]) => (
          <optgroup key={app} label={app}>
            {list.map((o) => (
              <option key={o.id} value={o.id}>
                {o.app ? `${o.app.name} / ${o.name}` : o.name}
              </option>
            ))}
          </optgroup>
        ))}
      </Select>
    </Field>
  );
}

/** `Project / Environment` of a store, linking to the environment in the Project Explorer; **Shared** without one. */
export function StoreOwner({ store }: { store: Pick<ObjectStore, "environment_id"> }) {
  const envs = useEnvironments();
  if (!store.environment_id) {
    return (
      <span className="text-faint" title="A shared store: not part of an environment">
        Shared
      </span>
    );
  }
  const env = envs.options.find((o) => o.id === store.environment_id);
  if (!env) {
    // Loading, failed, or an environment the caller cannot view.
    return (
      <span className="text-faint" title={envs.error ? errorText(envs.error) : `Environment ${store.environment_id}`}>
        {envs.isLoading ? "…" : envs.error ? "Environment unavailable" : "Another environment"}
      </span>
    );
  }
  const label = env.app ? `${env.app.name} / ${env.name}` : env.name;
  return env.app ? (
    <Link to="/projects" search={{ sel: `env:${env.id}` }} className="block truncate hover:text-accent" title={`Project ${env.app.name}, environment ${env.name}`}>
      {label}
    </Link>
  ) : (
    <span className="block truncate" title={`Environment ${env.name}`}>
      {label}
    </span>
  );
}

/** Assign a store to an environment, or make it shared again. Mount it while open. */
export function ChangeEnvironmentDialog({ store, onClose }: { store: ObjectStore; onClose: () => void }) {
  const save = useSetStoreOwner();
  const toast = useToast();
  const [environmentId, setEnvironmentId] = useState(store.environment_id ?? "");
  const unchanged = environmentId === (store.environment_id ?? "");
  // The server refuses to assign a store whose buckets other environments' services use, and names them.
  const blocked = save.error instanceof ApiError && save.error.code === ATTACHED_ELSEWHERE ? save.error : undefined;
  const submit = (force: boolean) =>
    save.mutate(
      { id: store.id, environment_id: environmentId || null, force },
      {
        onSuccess: () => {
          toast(environmentId ? `${store.name} assigned to the environment` : `${store.name} is a shared store now`);
          onClose();
        },
      },
    );
  if (blocked) {
    return (
      <Confirm
        open
        onOpenChange={(o) => !o && !save.isPending && save.reset()}
        title={`Assign ${store.name} anyway?`}
        body={
          <div className="space-y-2 text-[13px]">
            <p className="break-words">{blocked.message}</p>
            <p className="text-muted">
              People with a role on the new environment will be able to read, change and delete the objects in those buckets. Assign a store to an environment only when all its buckets belong to that environment.
            </p>
          </div>
        }
        confirmLabel="Assign anyway"
        busy={save.isPending}
        onConfirm={() => submit(true)}
      />
    );
  }
  return (
    <Modal open onOpenChange={(o) => !o && !save.isPending && onClose()} title="Change environment" description={`Which environment ${store.name} belongs to.`}>
      <form
        className="space-y-3"
        onSubmit={(e) => {
          e.preventDefault();
          submit(false);
        }}
      >
        <EnvironmentField value={environmentId} onChange={(v) => (save.reset(), setEnvironmentId(v))} />
        <p className="text-xs text-muted">
          People with a role on the environment can then browse <b>every</b> bucket of the store and attach buckets to its services, so choose an environment only when all the store's buckets belong to it. Buckets,
          objects, keys and attached services stay as they are. Managing the store still needs the storage.manage permission on its node or workspace.
        </p>
        <ErrorNote error={save.error} />
        <div className="flex justify-end gap-2 pt-1">
          <Button type="button" variant="ghost" onClick={onClose} disabled={save.isPending}>
            Cancel
          </Button>
          <Button variant="primary" disabled={save.isPending || unchanged} title={unchanged ? "Nothing to save" : undefined}>
            {save.isPending && <Loader2 className="size-4 animate-spin" />}
            Save
          </Button>
        </div>
      </form>
    </Modal>
  );
}

/** The object stores an environment owns. Renders nothing when it owns none. */
export function OwnedStores({ environmentId }: { environmentId: string }) {
  const stores = useObjectStores();
  const owned = ownedBy(stores.data, environmentId);
  if (stores.error && !stores.data) {
    return (
      <div className="border-t border-line px-4 py-2">
        <ErrorNote error={stores.error} />
      </div>
    );
  }
  if (!owned.length) return null;
  return (
    <div className="border-t border-line px-4 py-2">
      <div className="mb-1 text-[11px] font-semibold tracking-wide text-faint uppercase">Object stores</div>
      <ul className="space-y-1">
        {owned.map((s) => (
          <li key={s.id} className="flex min-w-0 items-center gap-2 text-[13px]">
            <Database className="size-3.5 shrink-0 text-muted" aria-hidden />
            <Link to="/storage/$storeId" params={{ storeId: s.id }} className="truncate font-medium hover:text-accent" title={s.name}>
              {s.name}
            </Link>
            <Status value={s.status} />
            {s.endpoint && (
              <span className="truncate font-mono text-[11px] text-faint" title={s.endpoint}>
                {s.endpoint}
              </span>
            )}
          </li>
        ))}
      </ul>
    </div>
  );
}

/** The stores of `list` that belong to the environment. */
export const ownedBy = (list: ObjectStore[] | undefined, environmentId: string) => (list ?? []).filter((s) => s.environment_id === environmentId);
