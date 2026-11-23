import { Link } from "@tanstack/react-router";
import { Copy, KeyRound, Loader2, Plus, RefreshCw, RotateCw, Trash2 } from "lucide-react";
import { useEffect, useState } from "react";

import { ApiError } from "../../api/client";
import { useBuckets, useCreateKey, useRevokeKey, useRotateKey, useStoreKeys } from "../../api/storage";
import type { KeyAccess, ObjectStore, StoreKey, StoreKeyIssued } from "../../api/types";
import { errorText, useToast } from "../../app/toast";
import { Badge, Button, Confirm, CopyButton, Empty, ErrorNote, Field, IconButton, Input, Modal, Panel, RowMenu, type RowAction, Select, Skeleton, Status, Table, Td, useRowMenu } from "../ui";

const KEY_NAME = "[A-Za-z0-9_.\\-]{1,32}";
const accessLabel = (a?: string | null) => (a === "read" ? "read" : a === "readwrite" ? "read-write" : null);
/** RustFS reports `on` / `off`; show them in the console's vocabulary. */
const keyStatus = (s?: string | null) => (s === "on" ? "enabled" : s === "off" ? "disabled" : s || "unknown");
const keyLabel = (k: StoreKey) => k.name || k.access_key;

/** Access keys limited to one bucket, issued by Forge on a managed RustFS store. Secrets stay in the vault. */
export function AccessKeys({ s, disabled }: { s: ObjectStore; disabled?: string }) {
  const keys = useStoreKeys(s.id, !disabled);
  const rotate = useRotateKey();
  const revoke = useRevokeKey();
  const toast = useToast();
  const [creating, setCreating] = useState(false);
  const [rotating, setRotating] = useState<StoreKey | null>(null);
  const [revoking, setRevoking] = useState<StoreKey | null>(null);
  const rm = useRowMenu();
  const list = keys.data ?? [];
  const keyActions = (k: StoreKey): RowAction[] =>
    k.managed_by_forge
      ? [
          { label: "Rotate", icon: RotateCw, onSelect: () => setRotating(k), disabled: rotate.isPending, hint: rotate.isPending ? "A rotation is running" : undefined },
          { label: "Revoke", icon: Trash2, danger: true, onSelect: () => setRevoking(k), disabled: revoke.isPending, hint: revoke.isPending ? "A revocation is running" : undefined },
        ]
      : [];
  // The page already handles a missing store, so a 404 here means an older server without the endpoint.
  const missing = keys.error instanceof ApiError && keys.error.status === 404;

  return (
    <Panel
      title="Access keys"
      description="Keys for services, each limited to one bucket. Secret keys are sealed in the Credential vault and never shown here."
      actions={
        <div className="flex gap-1.5">
          <IconButton label="Refresh keys" disabled={!!disabled} onClick={() => void keys.refetch()}>
            <RefreshCw className={`size-3.5 ${keys.isFetching ? "animate-spin" : ""}`} />
          </IconButton>
          <Button variant="primary" disabled={!!disabled} title={disabled ?? "Issue a key for one bucket"} onClick={() => setCreating(true)}>
            <Plus className="size-4" /> New key
          </Button>
        </div>
      }
    >
      {disabled ? (
        <p className="px-4 py-3 text-xs text-muted">{disabled}</p>
      ) : keys.isLoading ? (
        <Skeleton rows={2} />
      ) : missing ? (
        <p className="px-4 py-3 text-xs text-muted">This server does not manage access keys yet. Update Forge to use them.</p>
      ) : keys.error ? (
        <ErrorNote error={keys.error} />
      ) : !list.length ? (
        <Empty icon={<KeyRound className="size-5" />} title="No access keys" action={<Button onClick={() => setCreating(true)}><Plus className="size-4" /> New key</Button>}>
          Issue a key per service instead of sharing the root credential.
        </Empty>
      ) : (
        <Table head={["Name", "Access key", "Bucket", "Access", "Status", "Source", ""]}>
          {list.map((k) => (
            <tr key={k.access_key} className="text-[13px] hover:bg-panel-2/50" {...rm.rowProps(keyActions(k), `Actions for ${keyLabel(k)}`)}>
              <Td className="max-w-[14rem]">
                <span className="block truncate font-medium" title={[k.name, k.description].filter(Boolean).join(" · ") || undefined}>
                  {k.name || <span className="text-faint">—</span>}
                </span>
              </Td>
              <Td className="max-w-[16rem]">
                <span className="flex min-w-0 items-center gap-1">
                  <span className="truncate font-mono text-[11px] select-all" title={k.access_key}>
                    {k.access_key}
                  </span>
                  <IconButton label={`Copy access key ${k.access_key}`} className="!size-6 shrink-0" onClick={() => void navigator.clipboard?.writeText(k.access_key).then(() => toast("Access key copied"))}>
                    <Copy className="size-3" />
                  </IconButton>
                </span>
              </Td>
              <Td className="max-w-[12rem]">
                <span className="block truncate font-mono text-[11px]" title={k.bucket ?? undefined}>
                  {k.bucket || <span className="text-faint">—</span>}
                </span>
              </Td>
              <Td className="text-xs whitespace-nowrap">{accessLabel(k.access) ?? <span className="text-faint">—</span>}</Td>
              <Td title={k.expiration ? `Expires ${k.expiration}` : undefined}>
                <Status value={keyStatus(k.status)} />
              </Td>
              <Td className="whitespace-nowrap">
                {k.managed_by_forge ? (
                  <Badge tone="accent">Forge</Badge>
                ) : (
                  <span title="Created outside Forge, e.g. in the RustFS console. Manage it there.">
                    <Badge>Store</Badge>
                  </span>
                )}
              </Td>
              <Td className="w-10 text-right">
                {k.managed_by_forge ? (
                  <RowMenu label={`Actions for ${keyLabel(k)}`} actions={keyActions(k)} />
                ) : (
                  <span className="text-[11px] whitespace-nowrap text-faint" title="Not issued by Forge: rotate or revoke it in the RustFS console.">
                    at the store
                  </span>
                )}
              </Td>
            </tr>
          ))}
        </Table>
      )}
      {rm.menu}

      <NewKeyDialog open={creating} onOpenChange={setCreating} s={s} />
      <Confirm
        open={!!rotating}
        onOpenChange={(o) => !o && setRotating(null)}
        title={`Rotate key ${rotating ? keyLabel(rotating) : ""}?`}
        body={
          <div className="space-y-2">
            <p>Creates a new key, tests it on the bucket, then deletes the old one. Services using the old key must be updated.</p>
            <p className="text-xs text-muted">
              Old key <span className="font-mono text-[11px] text-fg">{rotating?.access_key}</span> on bucket <span className="font-mono text-[11px] text-fg">{rotating?.bucket ?? "—"}</span>. If the new key does not work, the old one stays. The new secret replaces the old one in the
              Credential vault.
            </p>
          </div>
        }
        confirmLabel="Rotate"
        busy={rotate.isPending}
        onConfirm={() => {
          const k = rotating;
          if (!k) return;
          rotate.mutate(
            { id: s.id, accessKey: k.access_key },
            {
              onSuccess: (r) => {
                setRotating(null);
                toast(`Rotated ${keyLabel(k)}${r?.access_key ? `: new access key ${r.access_key}` : ""}. The secret is in the Credential vault.`);
              },
              onError: (e) => toast(errorText(e), "error"),
            },
          );
        }}
      />
      <Confirm
        open={!!revoking}
        onOpenChange={(o) => !o && setRevoking(null)}
        title={`Revoke key ${revoking ? keyLabel(revoking) : ""}?`}
        body={
          <div className="space-y-2">
            <p>
              Deletes <span className="font-mono text-[11px] text-fg">{revoking?.access_key}</span> at the store and removes its secret from the Credential vault.
            </p>
            <p className="text-fg">
              Services using it lose access to bucket <span className="font-mono text-[11px]">{revoking?.bucket ?? "—"}</span> immediately. This cannot be undone.
            </p>
          </div>
        }
        confirmLabel="Revoke"
        busy={revoke.isPending}
        onConfirm={() => {
          const k = revoking;
          if (!k) return;
          revoke.mutate(
            { id: s.id, accessKey: k.access_key },
            {
              onSuccess: () => {
                setRevoking(null);
                toast(`Revoked ${keyLabel(k)}`);
              },
              onError: (e) => toast(errorText(e), "error"),
            },
          );
        }}
      />
    </Panel>
  );
}

function NewKeyDialog({ open, onOpenChange, s }: { open: boolean; onOpenChange: (o: boolean) => void; s: ObjectStore }) {
  const buckets = useBuckets(open ? s.id : undefined);
  const create = useCreateKey();
  const toast = useToast();
  const [name, setName] = useState("");
  const [bucket, setBucket] = useState("");
  const [access, setAccess] = useState<KeyAccess>("readwrite");
  const [issued, setIssued] = useState<StoreKeyIssued | null>(null);
  const list = buckets.data ?? [];

  useEffect(() => {
    if (open) {
      setName("");
      setBucket("");
      setAccess("readwrite");
      setIssued(null);
      create.reset();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  // Preselect the only bucket.
  useEffect(() => {
    if (open && !bucket && list.length === 1 && list[0]) setBucket(list[0].name);
  }, [open, bucket, list]);

  return (
    <Modal open={open} onOpenChange={onOpenChange} title={issued ? "Key created" : "New access key"} description={issued ? undefined : `A key for one bucket on ${s.name}.`}>
      {issued ? (
        <div className="space-y-3">
          <dl className="grid grid-cols-[6rem_minmax(0,1fr)] gap-x-3 gap-y-1.5 text-[13px]">
            <dt className="text-muted">Name</dt>
            <dd className="truncate">{issued.name || name}</dd>
            <dt className="text-muted">Access key</dt>
            <dd className="flex min-w-0 items-center gap-2">
              <span className="truncate font-mono text-xs select-all" title={issued.access_key}>
                {issued.access_key || "—"}
              </span>
              {issued.access_key && <CopyButton text={issued.access_key} label="Copy" />}
            </dd>
            <dt className="text-muted">Bucket</dt>
            <dd className="truncate font-mono text-xs">{issued.bucket || bucket}</dd>
            <dt className="text-muted">Access</dt>
            <dd className="text-xs">{accessLabel(issued.access ?? access) ?? "—"}</dd>
            <dt className="text-muted">Endpoint</dt>
            <dd className="truncate font-mono text-xs select-all" title={s.endpoint ?? undefined}>
              {s.endpoint || "—"}
            </dd>
          </dl>
          <p className="rounded-md border border-line px-3 py-2 text-xs text-muted">
            The secret key is stored in the{" "}
            <Link to="/vault" className="text-accent hover:underline" onClick={() => onOpenChange(false)}>
              Credential vault
            </Link>
            . Revealing it needs the <span className="font-mono text-[11px]">secret.read</span> permission and an MFA-verified sign-in.
          </p>
          <div className="flex justify-end">
            <Button variant="primary" onClick={() => onOpenChange(false)}>
              Done
            </Button>
          </div>
        </div>
      ) : (
        <form
          className="space-y-3"
          autoComplete="off"
          onSubmit={(e) => {
            e.preventDefault();
            create.mutate(
              { id: s.id, name: name.trim(), bucket, access },
              {
                onSuccess: (r) => {
                  setIssued(r ?? { access_key: "" });
                  toast(`Created key ${name.trim()}`);
                },
                onError: (err) => toast(errorText(err), "error"),
              },
            );
          }}
        >
          <Field label="Name" hint="What the key is for, e.g. the service. 1–32 letters, digits, - _ .">
            <Input autoFocus required maxLength={32} pattern={KEY_NAME} className="font-mono" value={name} placeholder="gitea" onChange={(e) => setName(e.target.value)} />
          </Field>
          <div className="grid gap-3 sm:grid-cols-2">
            <Field
              label="Bucket"
              hint={buckets.error ? undefined : !buckets.isLoading && !list.length ? "This store has no buckets. Create one in the bucket list first." : "The key works on this bucket only."}
            >
              <Select required className="font-mono" value={bucket} disabled={buckets.isLoading || !list.length} onChange={(e) => setBucket(e.target.value)}>
                <option value="">{buckets.isLoading ? "Loading buckets…" : list.length ? "Choose a bucket" : "No buckets"}</option>
                {list.map((b) => (
                  <option key={b.name} value={b.name}>
                    {b.name}
                  </option>
                ))}
              </Select>
            </Field>
            <Field label="Access" hint={access === "read" ? "List and download objects." : "List, download, upload and delete objects."}>
              <Select value={access} onChange={(e) => setAccess(e.target.value === "read" ? "read" : "readwrite")}>
                <option value="readwrite">read-write</option>
                <option value="read">read</option>
              </Select>
            </Field>
          </div>
          {buckets.error && <ErrorNote error={buckets.error} />}
          <ErrorNote error={create.error} />
          <div className="flex justify-end gap-2">
            <Button type="button" variant="ghost" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button variant="primary" disabled={create.isPending || !bucket || !name.trim()}>
              {create.isPending && <Loader2 className="size-4 animate-spin" />}
              Create key
            </Button>
          </div>
        </form>
      )}
    </Modal>
  );
}
