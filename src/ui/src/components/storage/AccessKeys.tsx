import { Link } from "@tanstack/react-router";
import { AlertTriangle, Copy, GitCompareArrows, KeyRound, Loader2, Pencil, Plus, RefreshCw, RotateCw, Trash2, X } from "lucide-react";
import { useEffect, useMemo, useState } from "react";

import { ApiError } from "../../api/client";
import { useBuckets, useCreateIamKey, useCreateKey, useReconcileIam, useRevokeKey, useRotateKey, useStoreIam, useStoreKeys, useUpdateIamKey } from "../../api/storage";
import type { IamDrift, IamGrant, IamKey, KeyAccess, ObjectStore, StoreKey } from "../../api/types";
import { errorText, useToast } from "../../app/toast";
import { Badge, Button, Confirm, CopyButton, Empty, ErrorNote, Field, IconButton, Input, Modal, Panel, RowMenu, type RowAction, Select, Skeleton, Status, Table, Td, useRowMenu } from "../ui";

const KEY_NAME = "[A-Za-z0-9_.\\-]{1,32}";
const MAX_GRANTS = 32;
const accessLabel = (a?: string | null) => (a === "read" ? "read" : a === "readwrite" ? "read-write" : null);
/** RustFS reports `on` / `off`; show them in the console's vocabulary. */
const keyStatus = (s?: string | null) => (s === "on" ? "enabled" : s === "off" ? "disabled" : s || "unknown");
const keyLabel = (k: Pick<IamKey, "name" | "access_key">) => k.name || k.access_key;

const DRIFT: Record<IamDrift, { label: string; tone: string; hint: string }> = {
  in_sync: { label: "in sync", tone: "ok", hint: "RustFS holds the policy Forge expects." },
  policy_changed: { label: "policy changed in RustFS", tone: "warn", hint: "Someone changed the policy in RustFS. Reconcile to accept it, or edit the grants to overwrite it." },
  missing_in_rustfs: { label: "missing in RustFS", tone: "bad", hint: "Forge knows this key but RustFS does not have it any more. Reconcile to forget it." },
  not_in_forge: { label: "not created by Forge", tone: "muted", hint: "Created outside Forge, e.g. in the RustFS console. You can still edit its grants." },
  unknown: { label: "unknown", tone: "muted", hint: "RustFS could not be asked; this is Forge's last known state." },
};

const grantText = (g: IamGrant) => `${g.bucket}/${g.prefix}`;

/** Legacy keys (one bucket each) in the IAM shape, for servers without the IAM endpoint. */
const fromLegacy = (k: StoreKey): IamKey => ({
  access_key: k.access_key,
  name: k.name ?? null,
  description: k.description ?? null,
  status: k.status ?? null,
  grants: k.bucket ? [{ bucket: k.bucket, prefix: "", access: k.access === "read" ? "read" : "readwrite" }] : [],
  custom_policy: false,
  managed_by_forge: k.managed_by_forge,
  record_id: null,
  vault_item_id: k.vault_item_id ?? null,
  drift: "unknown",
});

function Grants({ k }: { k: IamKey }) {
  if (!k.grants.length) return k.custom_policy ? null : <span className="text-xs text-faint">—</span>;
  return (
    <div className="space-y-0.5">
      {k.grants.map((g, i) => (
        <div key={`${grantText(g)}-${i}`} className="flex min-w-0 items-center gap-1.5 font-mono text-[11px]">
          <span className="min-w-0 truncate" title={grantText(g)}>
            {g.bucket}
            {g.prefix && <span className="text-muted">/{g.prefix}</span>}
          </span>
          <span className="shrink-0 text-faint">{accessLabel(g.access)}</span>
        </div>
      ))}
    </div>
  );
}

/** Access keys on a managed RustFS store, read from RustFS, with their grants. Secrets stay in the vault. */
export function AccessKeys({ s, disabled }: { s: ObjectStore; disabled?: string }) {
  const iam = useStoreIam(s.id, !disabled);
  const legacy = iam.error instanceof ApiError && iam.error.status === 404;
  const old = useStoreKeys(s.id, !disabled && legacy);
  const rotate = useRotateKey();
  const revoke = useRevokeKey();
  const reconcile = useReconcileIam();
  const toast = useToast();
  const [creating, setCreating] = useState(false);
  const [editing, setEditing] = useState<IamKey | null>(null);
  const [rotating, setRotating] = useState<IamKey | null>(null);
  const [revoking, setRevoking] = useState<IamKey | null>(null);
  const [reconciling, setReconciling] = useState(false);
  const rm = useRowMenu();

  const list = useMemo(() => (legacy ? (old.data ?? []).map(fromLegacy) : (iam.data?.keys ?? [])), [legacy, old.data, iam.data]);
  const reachable = legacy || iam.data?.reachable !== false;
  const readOnly = !reachable ? "RustFS is unreachable: showing Forge's last known state, read-only." : undefined;
  const q = legacy ? old : iam;
  const oldMissing = legacy && old.error instanceof ApiError && old.error.status === 404;

  const keyActions = (k: IamKey): RowAction[] => [
    {
      label: "Edit grants…",
      icon: Pencil,
      hidden: legacy,
      disabled: !!readOnly || k.drift === "missing_in_rustfs",
      hint: readOnly ?? (k.drift === "missing_in_rustfs" ? "The key no longer exists in RustFS" : undefined),
      onSelect: () => setEditing(k),
    },
    { label: "Rotate", icon: RotateCw, hidden: !k.managed_by_forge, onSelect: () => setRotating(k), disabled: rotate.isPending || !!readOnly, hint: readOnly ?? (rotate.isPending ? "A rotation is running" : undefined) },
    { label: "Revoke", icon: Trash2, danger: true, hidden: !k.managed_by_forge, onSelect: () => setRevoking(k), disabled: revoke.isPending || !!readOnly, hint: readOnly ?? (revoke.isPending ? "A revocation is running" : undefined) },
  ];
  const hasActions = (k: IamKey) => keyActions(k).some((a) => !a.hidden);

  const runReconcile = () =>
    reconcile.mutate(
      { id: s.id },
      {
        onSuccess: (r) => {
          setReconciling(false);
          const n = (v: unknown) => (typeof v === "number" ? v : 0);
          toast(`Reconciled from RustFS: ${n(r?.imported)} imported · ${n(r?.updated)} updated · ${n(r?.removed)} removed`);
        },
        onError: (e) => toast(errorText(e), "error"),
      },
    );

  return (
    <Panel
      title="Access keys"
      description="Keys for services and the buckets they reach, as RustFS holds them. Secret keys are sealed in the Credential vault and never shown here."
      actions={
        <div className="flex flex-wrap gap-1.5">
          <IconButton label="Refresh keys" disabled={!!disabled} onClick={() => void q.refetch()}>
            <RefreshCw className={`size-3.5 ${q.isFetching ? "animate-spin" : ""}`} />
          </IconButton>
          {!legacy && (
            <Button disabled={!!disabled || !!readOnly || reconcile.isPending} title={disabled ?? readOnly ?? "Accept what RustFS holds: import, update and forget keys in Forge"} onClick={() => setReconciling(true)}>
              {reconcile.isPending ? <Loader2 className="size-4 animate-spin" /> : <GitCompareArrows className="size-4" />}
              Reconcile from RustFS
            </Button>
          )}
          <Button variant="primary" disabled={!!disabled || !!readOnly} title={disabled ?? readOnly ?? "Issue a key for one or more buckets"} onClick={() => setCreating(true)}>
            <Plus className="size-4" /> New key
          </Button>
        </div>
      }
    >
      {!disabled && !reachable && (
        <div className="flex items-start gap-2 border-b border-line bg-warn-soft px-4 py-2 text-xs text-warn">
          <AlertTriangle className="mt-0.5 size-3.5 shrink-0" />
          <span className="min-w-0 break-words">
            RustFS is unreachable, so this is Forge's last known state and changes are off.
            {iam.data?.error && <span className="ml-1 font-mono text-[11px] select-all">{iam.data.error}</span>}
          </span>
        </div>
      )}
      {disabled ? (
        <p className="px-4 py-3 text-xs text-muted">{disabled}</p>
      ) : q.isLoading || (legacy && old.isPending) ? (
        <Skeleton rows={2} />
      ) : oldMissing ? (
        <p className="px-4 py-3 text-xs text-muted">This server does not manage access keys yet. Update Forge to use them.</p>
      ) : q.error ? (
        <ErrorNote error={q.error} />
      ) : !list.length ? (
        <Empty
          icon={<KeyRound className="size-5" />}
          title="No access keys"
          action={
            readOnly ? undefined : (
              <Button onClick={() => setCreating(true)}>
                <Plus className="size-4" /> New key
              </Button>
            )
          }
        >
          Issue a key per service instead of sharing the root credential.
        </Empty>
      ) : (
        <Table head={["Name", "Access key", "Grants", "Status", "Drift", "Source", ""]}>
          {list.map((k) => {
            const drift = DRIFT[k.drift] ?? DRIFT.unknown;
            return (
              <tr key={k.access_key} className="text-[13px] hover:bg-panel-2/50" {...(hasActions(k) ? rm.rowProps(keyActions(k), `Actions for ${keyLabel(k)}`) : {})}>
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
                <Td className="max-w-[18rem]">
                  <Grants k={k} />
                  {k.custom_policy && (
                    <span title="RustFS holds a policy that cannot be shown as bucket grants. Editing the grants replaces it.">
                      <Badge tone="warn">custom policy</Badge>
                    </span>
                  )}
                </Td>
                <Td>
                  <Status value={keyStatus(k.status)} />
                </Td>
                <Td className="whitespace-nowrap">
                  {legacy ? (
                    <span className="text-xs text-faint">—</span>
                  ) : (
                    <span title={drift.hint}>
                      <Badge tone={drift.tone}>{drift.label}</Badge>
                    </span>
                  )}
                </Td>
                <Td className="whitespace-nowrap">
                  {k.managed_by_forge ? (
                    <Badge tone="accent">Forge</Badge>
                  ) : (
                    <span title="Created outside Forge, e.g. in the RustFS console. Rotate or revoke it there.">
                      <Badge>Store</Badge>
                    </span>
                  )}
                </Td>
                <Td className="w-10 text-right">
                  {hasActions(k) ? (
                    <RowMenu label={`Actions for ${keyLabel(k)}`} actions={keyActions(k)} />
                  ) : (
                    <span className="text-[11px] whitespace-nowrap text-faint" title="Not issued by Forge: rotate or revoke it in the RustFS console.">
                      at the store
                    </span>
                  )}
                </Td>
              </tr>
            );
          })}
        </Table>
      )}
      {rm.menu}

      <KeyDialog open={creating} onOpenChange={setCreating} s={s} legacy={legacy} />
      <KeyDialog open={!!editing} onOpenChange={(o) => !o && setEditing(null)} s={s} edit={editing} />
      <Confirm
        open={reconciling}
        onOpenChange={setReconciling}
        title="Reconcile keys from RustFS?"
        body={
          <div className="space-y-2">
            <p>Forge accepts what RustFS holds: keys created in RustFS are imported, changed policies are taken over, and keys missing in RustFS are forgotten.</p>
            <p className="text-xs text-muted">Nothing changes in RustFS. Secrets of imported keys are not known to Forge and are not added to the vault.</p>
          </div>
        }
        confirmLabel="Reconcile"
        busy={reconcile.isPending}
        onConfirm={runReconcile}
      />
      <Confirm
        open={!!rotating}
        onOpenChange={(o) => !o && setRotating(null)}
        title={`Rotate key ${rotating ? keyLabel(rotating) : ""}?`}
        body={
          <div className="space-y-2">
            <p>Creates a new key with the same grants, tests it, then deletes the old one. Services using the old key must be updated.</p>
            <p className="text-xs text-muted">
              Old key <span className="font-mono text-[11px] text-fg">{rotating?.access_key}</span>. If the new key does not work, the old one stays. The new secret replaces the old one in the Credential vault.
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
              Services using it lose access to <span className="font-mono text-[11px]">{revoking?.grants.map(grantText).join(", ") || "its buckets"}</span> immediately. This cannot be undone.
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

interface GrantRow extends IamGrant {
  id: number;
}

let grantSeq = 0;
const grantRow = (g?: Partial<IamGrant>): GrantRow => ({ id: ++grantSeq, bucket: g?.bucket ?? "", prefix: g?.prefix ?? "", access: g?.access === "read" ? "read" : "readwrite" });

/** Prefix inside a bucket: relative, no wildcards; ends in `/` to mean a folder. */
const PREFIX = /^[^/*?\s][^*?]{0,1023}$/;

function grantProblems(rows: GrantRow[]): string[] {
  const p: string[] = [];
  if (!rows.length) p.push("Add at least one grant.");
  if (rows.some((r) => !r.bucket)) p.push("Choose a bucket for every grant.");
  if (rows.some((r) => r.prefix && !PREFIX.test(r.prefix))) p.push("Prefix: relative to the bucket (no leading /), no * or ?, e.g. uploads/.");
  const keys = rows.map((r) => `${r.bucket}/${r.prefix}`);
  if (new Set(keys).size !== keys.length) p.push("Each bucket and prefix once.");
  return p;
}

/** New key with one or more grants, or (with `edit`) the grants of an existing key. `legacy`: older server, one bucket, no prefix. */
function KeyDialog({ open, onOpenChange, s, edit, legacy }: { open: boolean; onOpenChange: (o: boolean) => void; s: ObjectStore; edit?: IamKey | null; legacy?: boolean }) {
  const buckets = useBuckets(open ? s.id : undefined);
  const create = useCreateIamKey();
  const createOld = useCreateKey();
  const update = useUpdateIamKey();
  const toast = useToast();
  const [name, setName] = useState("");
  const [rows, setRows] = useState<GrantRow[]>([]);
  const [issued, setIssued] = useState<{ access_key: string; name?: string; grants?: IamGrant[] } | null>(null);
  const bucketNames = (buckets.data ?? []).map((b) => b.name).filter(Boolean);
  const pending = create.isPending || createOld.isPending || update.isPending;
  const error = create.error ?? createOld.error ?? update.error;

  useEffect(() => {
    if (open) {
      setName(edit?.name ?? "");
      setRows(edit?.grants.length ? edit.grants.map(grantRow) : [grantRow()]);
      setIssued(null);
      create.reset();
      createOld.reset();
      update.reset();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  // Preselect the only bucket on a fresh row.
  useEffect(() => {
    if (open && !edit && bucketNames.length === 1 && rows.length === 1 && rows[0] && !rows[0].bucket) setRows([{ ...rows[0], bucket: bucketNames[0] ?? "" }]);
  }, [open, edit, bucketNames, rows]);

  const setRow = (id: number, p: Partial<GrantRow>) => setRows((l) => l.map((r) => (r.id === id ? { ...r, ...p } : r)));
  const problems = grantProblems(rows);
  const grants: IamGrant[] = rows.map(({ bucket, prefix, access }) => ({ bucket, prefix: prefix.trim(), access }));
  // Buckets a key already holds that RustFS no longer lists stay choosable.
  const choices = [...new Set([...bucketNames, ...rows.map((r) => r.bucket).filter(Boolean)])];

  const created = (r?: { access_key?: string; name?: string } | null) => {
    setIssued({ access_key: r?.access_key ?? "", name: r?.name, grants });
    toast(`Created key ${name.trim()}`);
  };

  const onSubmit = () => {
    const done = { onError: (err: unknown) => toast(errorText(err), "error") };
    if (edit) {
      update.mutate(
        { id: s.id, accessKey: edit.access_key, grants },
        {
          ...done,
          onSuccess: () => {
            toast(`Updated grants of ${keyLabel(edit)}`);
            onOpenChange(false);
          },
        },
      );
    } else if (legacy) {
      const g = grants[0];
      if (!g) return;
      createOld.mutate({ id: s.id, name: name.trim(), bucket: g.bucket, access: g.access }, { ...done, onSuccess: (r) => created(r) });
    } else {
      create.mutate({ id: s.id, name: name.trim(), grants }, { ...done, onSuccess: (r) => created(r) });
    }
  };

  return (
    <Modal
      open={open}
      onOpenChange={onOpenChange}
      title={issued ? "Key created" : edit ? `Grants of ${keyLabel(edit)}` : "New access key"}
      description={issued ? undefined : edit ? "Saving writes the policy to RustFS; the key works with the new grants immediately." : `A key for one or more buckets on ${s.name}.`}
      wide
    >
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
            <dt className="text-muted">Grants</dt>
            <dd className="space-y-0.5 font-mono text-[11px]">
              {(issued.grants ?? []).map((g, i) => (
                <div key={i} className="truncate">
                  {grantText(g)} <span className="text-faint">{accessLabel(g.access)}</span>
                </div>
              ))}
            </dd>
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
            if (!problems.length) onSubmit();
          }}
        >
          {!edit && (
            <Field label="Name" hint="What the key is for, e.g. the service. 1–32 letters, digits, - _ .">
              <Input autoFocus required maxLength={32} pattern={KEY_NAME} className="font-mono" value={name} placeholder="gitea" onChange={(e) => setName(e.target.value)} />
            </Field>
          )}
          {edit?.custom_policy && <p className="rounded-md bg-warn-soft px-3 py-2 text-xs text-warn">RustFS holds a custom policy for this key. Saving replaces it with these grants.</p>}
          <div className="space-y-1">
            <div className="text-xs font-medium text-muted">Grants</div>
            {rows.map((r, i) => (
              <div key={r.id} className="flex min-w-0 flex-wrap items-center gap-1.5 sm:flex-nowrap">
                <Select aria-label={`Grant ${i + 1} bucket`} required className="min-w-0 flex-1 font-mono !text-xs" value={r.bucket} disabled={buckets.isLoading} onChange={(e) => setRow(r.id, { bucket: e.target.value })}>
                  <option value="">{buckets.isLoading ? "Loading buckets…" : choices.length ? "Choose a bucket" : "No buckets"}</option>
                  {choices.map((b) => (
                    <option key={b} value={b}>
                      {b}
                    </option>
                  ))}
                </Select>
                <Input
                  aria-label={`Grant ${i + 1} prefix`}
                  className="min-w-0 flex-1 font-mono !text-xs"
                  value={r.prefix}
                  disabled={legacy}
                  title={legacy ? "This server grants whole buckets only" : "Optional, e.g. uploads/. Empty = the whole bucket."}
                  placeholder={legacy ? "whole bucket" : "prefix (optional)"}
                  onChange={(e) => setRow(r.id, { prefix: e.target.value })}
                />
                <Select aria-label={`Grant ${i + 1} access`} className="!w-32 !text-xs" value={r.access} onChange={(e) => setRow(r.id, { access: (e.target.value === "read" ? "read" : "readwrite") as KeyAccess })}>
                  <option value="readwrite">read-write</option>
                  <option value="read">read</option>
                </Select>
                <IconButton type="button" label={`Remove grant ${i + 1}`} disabled={rows.length <= 1} onClick={() => setRows((l) => l.filter((x) => x.id !== r.id))}>
                  <X className="size-3.5" />
                </IconButton>
              </div>
            ))}
            {!legacy && (
              <Button type="button" variant="ghost" disabled={rows.length >= MAX_GRANTS} onClick={() => setRows((l) => [...l, grantRow()])}>
                <Plus className="size-4" /> Add grant
              </Button>
            )}
            <p className="text-[11px] text-faint">read: list and download. read-write: also upload and delete. A prefix limits the grant to keys under it.</p>
            {!buckets.isLoading && !buckets.error && !bucketNames.length && <p className="text-xs text-muted">This store has no buckets. Create one in the bucket list first.</p>}
          </div>
          {buckets.error && <ErrorNote error={buckets.error} />}
          {problems.length > 0 && rows.some((r) => r.bucket) && (
            <ul className="space-y-0.5 text-xs text-bad">
              {problems.map((p) => (
                <li key={p}>{p}</li>
              ))}
            </ul>
          )}
          <ErrorNote error={error} />
          <div className="flex justify-end gap-2">
            <Button type="button" variant="ghost" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button variant="primary" disabled={pending || problems.length > 0 || (!edit && !name.trim())}>
              {pending && <Loader2 className="size-4 animate-spin" />}
              {edit ? "Save grants" : "Create key"}
            </Button>
          </div>
        </form>
      )}
    </Modal>
  );
}
