import { Eraser, KeyRound, Loader2, Plus, RotateCw, Trash2 } from "lucide-react";
import { useState } from "react";

import { type SwarmObject, type SwarmObjectKind, useCreateSwarmObject, usePruneSwarmObjects, useRemoveSwarmObject, useSwarmObjects } from "../../api/swarm";
import { errorText, useToast } from "../../app/toast";
import { Badge, Button, Confirm, Empty, ErrorNote, Field, Input, Modal, Panel, Segmented, Skeleton, TextArea, ago, time } from "../ui";

const NAME_RE = /^[A-Za-z0-9][A-Za-z0-9_.-]{0,62}$/;

/** Swarm secrets and configs, grouped by logical name with every version. */
export function SwarmObjects({ clusterId }: { clusterId: string }) {
  const [kind, setKind] = useState<SwarmObjectKind>("secret");
  const q = useSwarmObjects(clusterId, kind);
  const toast = useToast();
  const prune = usePruneSwarmObjects(clusterId);
  const [editing, setEditing] = useState<{ name?: string } | null>(null);
  const [removing, setRemoving] = useState<SwarmObject | null>(null);
  const [pruning, setPruning] = useState(false);
  const noun = kind === "secret" ? "secret" : "config";

  const groups = new Map<string, SwarmObject[]>();
  for (const o of q.data ?? []) groups.set(o.name, [...(groups.get(o.name) ?? []), o]);
  for (const list of groups.values()) list.sort((a, b) => b.version - a.version);

  return (
    <Panel
      title="Secrets & configs"
      description="Versioned: rotating creates the next version and services move to it on their next deploy."
      actions={
        <div className="flex flex-wrap items-center gap-2">
          <Segmented
            label="Kind"
            value={kind}
            onChange={setKind}
            options={[
              { id: "secret", label: "Secrets" },
              { id: "config", label: "Configs" },
            ]}
          />
          <Button onClick={() => setPruning(true)} disabled={!q.data?.length}>
            <Eraser className="size-4" /> Prune unused
          </Button>
          <Button variant="primary" onClick={() => setEditing({})}>
            <Plus className="size-4" /> New {noun}
          </Button>
        </div>
      }
    >
      {q.error ? (
        <div className="p-3">
          <ErrorNote error={q.error} />
        </div>
      ) : q.isLoading ? (
        <div className="p-3">
          <Skeleton rows={3} />
        </div>
      ) : groups.size === 0 ? (
        <Empty
          icon={<KeyRound className="size-5" />}
          title={`No ${noun}s`}
          action={
            <Button variant="primary" onClick={() => setEditing({})}>
              <Plus className="size-4" /> New {noun}
            </Button>
          }
        >
          Create a {noun} from a value or a vault item, then reference it from a stack's compose file as an external {noun}.
        </Empty>
      ) : (
        <ul className="divide-y divide-line text-[13px]">
          {[...groups.entries()].map(([name, versions]) => (
            <li key={name} className="px-3 py-2">
              <div className="flex min-w-0 items-center gap-2">
                <span className="truncate font-mono text-xs font-medium select-all" title={name}>
                  {name}
                </span>
                <Badge tone="muted">{versions.length} version{versions.length === 1 ? "" : "s"}</Badge>
                <Button className="ml-auto" onClick={() => setEditing({ name })}>
                  <RotateCw className="size-4" /> Rotate
                </Button>
              </div>
              <ul className="mt-1 space-y-0.5">
                {versions.map((o, i) => (
                  <li key={o.docker_name} className="flex min-w-0 flex-wrap items-center gap-2 text-xs">
                    <span className="w-10 font-mono text-[11px] text-muted tabular-nums">v{o.version || "—"}</span>
                    {i === 0 && <Badge tone="accent">latest</Badge>}
                    <span className="max-w-64 truncate font-mono text-[11px] text-faint select-all" title={o.docker_name}>
                      {o.docker_name}
                    </span>
                    <span className="text-faint" title={o.created_at ? time(o.created_at) : undefined}>
                      {o.created_at ? ago(o.created_at) : "—"}
                    </span>
                    <span className="min-w-0 truncate text-muted" title={o.used_by.join(", ")}>
                      {o.used_by.length ? `used by ${o.used_by.join(", ")}` : "unused"}
                    </span>
                    <button
                      type="button"
                      className="ml-auto rounded p-1 text-faint hover:text-bad focus-visible:outline-2 focus-visible:outline-accent disabled:opacity-40"
                      aria-label={`Remove ${o.docker_name}`}
                      title={o.used_by.length ? "In use: redeploy the services on a newer version first" : `Remove ${o.docker_name}`}
                      disabled={o.used_by.length > 0}
                      onClick={() => setRemoving(o)}
                    >
                      <Trash2 className="size-3.5" />
                    </button>
                  </li>
                ))}
              </ul>
            </li>
          ))}
        </ul>
      )}
      {editing && <ObjectDialog clusterId={clusterId} kind={kind} name={editing.name} onClose={() => setEditing(null)} />}
      {removing && <RemoveObject clusterId={clusterId} o={removing} onClose={() => setRemoving(null)} />}
      {pruning && (
        <Confirm
          open
          onOpenChange={(o) => !o && setPruning(false)}
          title={`Prune unused ${noun}s?`}
          confirmLabel="Prune"
          busy={prune.isPending}
          onConfirm={() =>
            prune.mutate(kind, {
              onSuccess: (removed) => {
                toast(removed.length ? `Removed ${removed.length} unused ${noun} version${removed.length === 1 ? "" : "s"}` : `Nothing to prune`);
                setPruning(false);
              },
              onError: (e) => toast(errorText(e), "error"),
            })
          }
          body={<p className="text-[13px]">Removes every older {noun} version that no service uses. The latest version of each name and anything in use stay.</p>}
        />
      )}
    </Panel>
  );
}

function RemoveObject({ clusterId, o, onClose }: { clusterId: string; o: SwarmObject; onClose: () => void }) {
  const rm = useRemoveSwarmObject(clusterId);
  const toast = useToast();
  return (
    <Confirm
      open
      onOpenChange={(x) => !x && onClose()}
      title={`Remove ${o.docker_name}?`}
      confirmLabel="Remove"
      busy={rm.isPending}
      onConfirm={() =>
        rm.mutate(
          { kind: o.kind, docker_name: o.docker_name },
          {
            onSuccess: () => {
              toast(`Removed ${o.docker_name}`);
              onClose();
            },
            onError: (e) => toast(errorText(e), "error"),
          },
        )
      }
      body={<p className="text-[13px]">Version {o.version} of {o.name} is deleted from the swarm. A stack that still names it fails its next deploy.</p>}
    />
  );
}

function ObjectDialog({ clusterId, kind, name, onClose }: { clusterId: string; kind: SwarmObjectKind; name?: string; onClose: () => void }) {
  const create = useCreateSwarmObject(clusterId);
  const toast = useToast();
  const [f, setF] = useState({ name: name ?? "", source: "value" as "value" | "vault", value: "", vault: "" });
  const rotating = !!name;
  const nameOk = NAME_RE.test(f.name);
  const ok = nameOk && (f.source === "value" ? f.value.length > 0 : f.vault.trim().length > 0);
  const noun = kind === "secret" ? "secret" : "config";

  return (
    <Modal
      open
      onOpenChange={(o) => !o && onClose()}
      title={rotating ? `Rotate ${name}` : `New ${noun}`}
      description={rotating ? "Creates the next version. Redeploy the stacks that use it to switch over." : `Creates version 1. Secrets are encrypted in the swarm's Raft log and mounted at /run/secrets.`}
    >
      <form
        className="space-y-3"
        onSubmit={(e) => {
          e.preventDefault();
          if (!ok) return;
          create.mutate(
            { kind, name: f.name, ...(f.source === "value" ? { value: f.value } : { vault_item_id: f.vault.trim() }) },
            {
              onSuccess: (r) => {
                toast(`${r.docker_name}${r.version ? ` (v${r.version})` : ""} created`);
                onClose();
              },
              onError: (e) => toast(errorText(e), "error"),
            },
          );
        }}
      >
        <Field label="Name" hint={f.name && !nameOk ? <span className="text-bad">Letters, digits, . _ -; at most 63</span> : "Referenced in compose files; Forge appends the version."}>
          <Input value={f.name} disabled={rotating} onChange={(e) => setF({ ...f, name: e.target.value.trim() })} className="font-mono" placeholder="db_password" autoFocus={!rotating} />
        </Field>
        {kind === "secret" && (
          <Segmented
            label="Source"
            value={f.source}
            onChange={(source) => setF({ ...f, source })}
            options={[
              { id: "value", label: "Value" },
              { id: "vault", label: "Vault item" },
            ]}
          />
        )}
        {f.source === "value" || kind === "config" ? (
          <Field label="Value" hint={kind === "secret" ? "Sent once to the swarm; Forge does not keep it." : undefined}>
            <TextArea rows={kind === "config" ? 8 : 3} value={f.value} onChange={(e) => setF({ ...f, value: e.target.value })} className="font-mono" autoFocus={rotating} spellCheck={false} autoComplete="off" />
          </Field>
        ) : (
          <Field label="Vault item id" hint="The item's password becomes the secret; the value never reaches the browser.">
            <Input value={f.vault} onChange={(e) => setF({ ...f, vault: e.target.value })} className="font-mono" placeholder="vault item id" />
          </Field>
        )}
        <div className="flex justify-end gap-2 pt-2">
          <Button type="button" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" disabled={!ok || create.isPending}>
            {create.isPending && <Loader2 className="size-4 animate-spin" />} {rotating ? "Rotate" : "Create"}
          </Button>
        </div>
      </form>
    </Modal>
  );
}
