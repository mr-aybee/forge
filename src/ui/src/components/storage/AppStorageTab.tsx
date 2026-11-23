// The service's Storage tab: buckets of managed object stores attached to it.
// Forge issues a key limited to the bucket, keeps the secret in the vault and
// writes five variables into the environment at the next deploy.
import { Link } from "@tanstack/react-router";
import { ChevronRight, Database, Info, Loader2, Plus, Unplug } from "lucide-react";
import { Fragment, useState } from "react";

import { ApiError } from "../../api/client";
import { useServiceStorage, useAttachServiceStorage, useDetachServiceStorage } from "../../api/hooks";
import { keysSupported, useBuckets, useObjectStores } from "../../api/storage";
import type { ServiceStorageAttachment, ServiceStorageEnv, Service, KeyAccess } from "../../api/types";
import { errorText, useToast } from "../../app/toast";
import { Badge, Button, Confirm, CopyIcon, CopyValue, Empty, ErrorNote, Field, Input, Modal, Select, Skeleton, Table, Td } from "../ui";

const ACCESS_LABEL: Record<KeyAccess, string> = { readwrite: "Read & write", read: "Read only" };

/** The five variables, in the order they are shown. */
const VARS: { key: keyof ServiceStorageEnv; label: string; suffix: string }[] = [
  { key: "endpoint", label: "Endpoint", suffix: "ENDPOINT" },
  { key: "region", label: "Region", suffix: "REGION" },
  { key: "bucket", label: "Bucket", suffix: "BUCKET" },
  { key: "access_key", label: "Access key ID", suffix: "ACCESS_KEY_ID" },
  { key: "secret_key", label: "Secret access key", suffix: "SECRET_ACCESS_KEY" },
];

const PREFIX_MAX = 24;

/** Mirrors the server's prefix rule; returns the problem, or undefined when valid. */
function prefixProblem(p: string): string | undefined {
  if (!p) return "Enter a prefix.";
  if (p.length > PREFIX_MAX) return `At most ${PREFIX_MAX} characters.`;
  if (!/^[A-Z]/.test(p)) return "Starts with an uppercase letter.";
  if (!/^[A-Z][A-Z0-9_]*$/.test(p)) return "Only uppercase letters, digits and underscores.";
  if (p.endsWith("_")) return "Must not end with an underscore.";
  return undefined;
}

const BUCKET_NAME = /^[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]$/;

const CANT_CHANGE = "You can view attachments but not change them (needs the deploy permission on this service's environment)";

/** A refused attach or detach, in words. Step-up itself is prompted by the API client, which retries once. */
const mutationText = (e: unknown) =>
  e instanceof ApiError && e.code === "step_up_required" ? `Not verified: ${e.message}. Changing attached storage needs an MFA-verified sign-in.` : e instanceof ApiError && e.status === 403 ? `Not allowed: ${e.message}` : errorText(e);

export function AppStorageTab({ app }: { app: Service }) {
  const q = useServiceStorage(app.id);
  const detach = useDetachServiceStorage();
  const toast = useToast();
  const [attaching, setAttaching] = useState(false);
  const [removing, setRemoving] = useState<ServiceStorageAttachment>();
  /** Access keys of the rows whose variable list is open. */
  const [open, setOpen] = useState<Set<string>>(() => new Set());

  if (q.isLoading) return <Skeleton rows={4} />;
  if (q.error || !q.data) {
    const e = q.error;
    const missing = e instanceof ApiError && e.status === 404;
    const denied = e instanceof ApiError && e.status === 403;
    return (
      <div className="rounded-lg border border-line">
        <Empty icon={<Database className="size-5" />} title={missing ? "Attached storage isn't available" : denied ? "You can't view attached storage" : "Couldn't load attached storage"}>
          {missing ? "This server doesn't support attaching buckets yet. Update Forge to use it." : denied ? "Viewing it needs access to this service." : <ErrorNote error={e ?? "No data"} />}
        </Empty>
      </div>
    );
  }

  const list = q.data.attachments;
  const canChange = q.data.can_change;
  const toggle = (key: string) =>
    setOpen((s) => {
      const n = new Set(s);
      if (!n.delete(key)) n.add(key);
      return n;
    });

  const attachButton = (
    <Button variant="primary" className="h-7 px-2 text-xs" disabled={!canChange} title={canChange ? "Attach a bucket of a managed object store" : CANT_CHANGE} onClick={() => setAttaching(true)}>
      <Plus className="size-3.5" /> Attach bucket
    </Button>
  );

  return (
    <div className="space-y-3 pb-2">
      <section aria-labelledby="as-title" className="rounded-lg border border-line">
        <header className="flex flex-wrap items-center gap-x-3 gap-y-1.5 border-b border-line px-3 py-2">
          <h3 id="as-title" className="text-[13px] font-semibold">
            Attached buckets
          </h3>
          <span className="min-w-0 flex-1 truncate text-[11px] text-muted">
            {list.length ? <span className="font-mono tabular-nums">{list.length}</span> : "none"} · each with its own key, limited to the bucket
          </span>
          {!canChange && <Badge tone="muted">View only</Badge>}
          {list.length > 0 && attachButton}
        </header>

        {list.length === 0 ? (
          <Empty icon={<Database className="size-5" />} title="No bucket attached" action={attachButton}>
            Attach a bucket of a managed object store. Forge issues a key limited to that bucket, keeps the secret in the vault and gives this service the endpoint, region, bucket and key as
            environment variables with its next deploy.
          </Empty>
        ) : (
          <Table head={["Store", "Bucket", "Access", "Endpoint", "Variables", ""]}>
            {list.map((a) => {
              const isOpen = open.has(a.access_key);
              const names = VARS.map((v) => ({ ...v, name: a.env[v.key] })).filter((v) => v.name);
              return (
                <Fragment key={a.access_key}>
                  <tr>
                    <Td className="max-w-[220px] !py-1.5">
                      {a.store_name ? (
                        <Link to="/storage/$storeId" params={{ storeId: a.store_id }} className="block truncate text-[13px] font-medium hover:text-accent" title={`Open store ${a.store_name}`}>
                          {a.store_name}
                        </Link>
                      ) : (
                        <span className="text-xs text-warn" title="The store was deleted. This attachment no longer works; detach it.">
                          store removed
                        </span>
                      )}
                    </Td>
                    <Td className="max-w-[220px] !py-1.5">
                      <span className="block truncate font-mono text-xs select-all" title={a.bucket}>
                        {a.bucket || "—"}
                      </span>
                    </Td>
                    <Td className="!py-1.5 whitespace-nowrap">
                      <Badge tone={a.access === "read" ? "muted" : "accent"}>{a.access === "read" ? "Read" : "Read & write"}</Badge>
                    </Td>
                    <Td className="max-w-[260px] !py-1.5">
                      <CopyValue text={a.endpoint ?? ""} label="Endpoint" />
                    </Td>
                    <Td className="!py-1.5 whitespace-nowrap">
                      <button
                        type="button"
                        aria-expanded={isOpen}
                        aria-label={`${isOpen ? "Hide" : "Show"} the variables of ${a.bucket}`}
                        onClick={() => toggle(a.access_key)}
                        className="-ml-1 inline-flex items-center gap-1 rounded px-1 font-mono text-[11px] text-muted hover:text-accent"
                        title="The names of the variables this service gets"
                      >
                        <ChevronRight className={`size-3.5 transition-transform ${isOpen ? "rotate-90" : ""}`} />
                        {a.prefix || "—"}_* <span className="text-faint tabular-nums">· {names.length}</span>
                      </button>
                    </Td>
                    <Td className="!py-1.5 text-right whitespace-nowrap">
                      <Button variant="ghost" className="h-7 px-2 text-xs hover:!text-bad" disabled={!canChange} title={canChange ? "Revoke the key and remove the variables" : CANT_CHANGE} onClick={() => setRemoving(a)}>
                        <Unplug className="size-3.5" /> Detach
                      </Button>
                    </Td>
                  </tr>
                  {isOpen && (
                    <tr className="bg-panel-2/40">
                      <td colSpan={6} className="px-4 py-2">
                        {names.length === 0 ? (
                          <span className="text-xs text-faint">—</span>
                        ) : (
                          <ul className="grid gap-x-6 gap-y-0.5 sm:grid-cols-2 xl:grid-cols-3">
                            {names.map((v) => (
                              <li key={v.key} className="flex min-w-0 items-center gap-2 text-xs">
                                <span className="w-28 shrink-0 text-muted">{v.label}</span>
                                <span className="min-w-0 truncate font-mono text-[11px] select-all" title={v.name}>
                                  {v.name}
                                </span>
                                <CopyIcon text={v.name} label={`Copy ${v.name}`} />
                              </li>
                            ))}
                          </ul>
                        )}
                        <p className="mt-1 text-[11px] text-faint">
                          Key <span className="font-mono select-all">{a.access_key}</span> · the secret is in the vault and is written only to the nodes.
                        </p>
                      </td>
                    </tr>
                  )}
                </Fragment>
              );
            })}
          </Table>
        )}
      </section>

      <p className="flex items-start gap-1.5 px-1 text-[11px] text-faint">
        <Info className="mt-px size-3.5 shrink-0" />
        <span>Variables are added at deploy. A variable the service defines itself wins. The secret stays in the vault.</span>
      </p>

      {attaching && <AttachDialog app={app} existing={list} onClose={() => setAttaching(false)} />}
      <Confirm
        open={!!removing}
        onOpenChange={(o) => !o && !detach.isPending && setRemoving(undefined)}
        title={`Detach bucket ${removing?.bucket ?? ""}?`}
        body={
          <div className="space-y-2 text-[13px]">
            <p>
              The key <span className="font-mono text-xs break-all">{removing?.access_key}</span> is revoked at once: {app.name} loses access to this bucket immediately, also while it is running.
            </p>
            <p>
              The <span className="font-mono text-xs">{removing?.prefix}_*</span> variables disappear with the next deploy. The bucket and its objects are kept.
            </p>
          </div>
        }
        confirmLabel="Detach"
        busy={detach.isPending}
        onConfirm={() =>
          removing &&
          detach.mutate(
            { id: app.id, accessKey: removing.access_key },
            {
              onSuccess: () => {
                toast(`Bucket ${removing.bucket} detached. Its key is revoked; the variables disappear with the next deploy.`);
                setRemoving(undefined);
              },
              onError: (e) => toast(mutationText(e), "error"),
            },
          )
        }
      />
    </div>
  );
}

/** "New bucket" in the bucket select; not a valid bucket name, so it can't collide. */
const NEW = "__new__";

interface AttachForm {
  store_id: string;
  /** An existing bucket's name, `NEW`, or "" while nothing is chosen. */
  bucket: string;
  new_name: string;
  access: KeyAccess;
  prefix: string;
}

function AttachDialog({ app, existing, onClose }: { app: Service; existing: ServiceStorageAttachment[]; onClose: () => void }) {
  const stores = useObjectStores();
  const attach = useAttachServiceStorage();
  const toast = useToast();
  const [form, setForm] = useState<AttachForm>({ store_id: "", bucket: "", new_name: "", access: "readwrite", prefix: "S3" });
  const set = (patch: Partial<AttachForm>) => (attach.reset(), setForm((f) => ({ ...f, ...patch })));

  // Stores of this service's environment first: attaching them needs no storage.manage.
  const own = (s: { environment_id: string | null }) => s.environment_id === app.environment_id;
  const attachable = (stores.data ?? []).filter(keysSupported).sort((a, b) => Number(own(b)) - Number(own(a)) || a.name.localeCompare(b.name));
  const ownStores = attachable.filter(own);
  const otherStores = attachable.filter((s) => !own(s));
  // A single attachable store is the choice.
  const storeId = form.store_id || (attachable.length === 1 ? (attachable[0]?.id ?? "") : "");
  const chosenStore = attachable.find((s) => s.id === storeId);
  const storeHint = !chosenStore
    ? "Managed RustFS stores. A store of this environment needs the deploy permission only."
    : own(chosenStore)
      ? "Belongs to this environment: the deploy permission is enough."
      : "Shared or another environment's store: attaching also needs the storage.manage permission on the store and an MFA-verified sign-in.";
  const buckets = useBuckets(storeId || undefined);
  const bucketList = buckets.data ?? [];
  // A store without buckets (or whose list failed) leaves only "New bucket".
  const choice = form.bucket || (storeId && !buckets.isLoading && bucketList.length === 0 ? NEW : "");
  const isNew = choice === NEW;
  const bucket = isNew ? form.new_name.trim() : choice;

  const names = VARS.map((v) => `${form.prefix}_${v.suffix}`);
  const defined = new Set((Array.isArray(app.env) ? app.env : []).map((e) => e.key));
  const clash = names.filter((n) => defined.has(n));
  const prefixError =
    prefixProblem(form.prefix) ??
    (existing.some((a) => a.prefix === form.prefix)
      ? "Another attachment of this service already uses this prefix."
      : clash.length
        ? `This service already defines ${clash.join(", ")}. Choose another prefix or remove the variable.`
        : undefined);
  const nameError = isNew && form.new_name && !BUCKET_NAME.test(bucket) ? "3–63 lowercase letters, digits, dots and hyphens; starts and ends with a letter or digit." : undefined;
  const blocked = !storeId ? "Choose a store" : !bucket ? (isNew ? "Name the new bucket" : "Choose a bucket") : nameError ? "Fix the bucket name" : prefixError ? "Fix the prefix" : undefined;

  const submit = () => {
    if (blocked) return;
    attach.mutate(
      { id: app.id, store_id: storeId, bucket, create_bucket: isNew || undefined, access: form.access, prefix: form.prefix },
      {
        onSuccess: () => {
          toast(`Bucket ${bucket} attached. ${app.name} gets the ${form.prefix}_* variables with its next deploy.`);
          onClose();
        },
      },
    );
  };

  return (
    <Modal open onOpenChange={(o) => !o && !attach.isPending && onClose()} title="Attach bucket" description={`Gives ${app.name} its own key for one bucket of a managed object store.`}>
      {stores.isLoading ? (
        <Skeleton rows={3} />
      ) : stores.error ? (
        <ErrorNote error={stores.error} />
      ) : attachable.length === 0 ? (
        <div className="space-y-4 text-[13px]">
          <p className="text-muted">
            No store can be attached. Only managed RustFS stores issue keys; connected stores can't.{" "}
            <Link to="/storage" className="text-accent hover:underline">
              Open Storage
            </Link>{" "}
            to install one.
          </p>
          <div className="flex justify-end">
            <Button onClick={onClose}>Close</Button>
          </div>
        </div>
      ) : (
        <form
          className="space-y-3"
          onSubmit={(e) => {
            e.preventDefault();
            submit();
          }}
        >
          <Field label="Store" hint={storeHint}>
            <Select aria-label="Store" value={storeId} onChange={(e) => set({ store_id: e.target.value, bucket: "", new_name: "" })}>
              {!storeId && <option value="">Choose a store…</option>}
              {ownStores.length > 0 && (
                <optgroup label="This environment">
                  {ownStores.map((s) => (
                    <option key={s.id} value={s.id}>
                      {s.name}
                    </option>
                  ))}
                </optgroup>
              )}
              {otherStores.length > 0 && (
                <optgroup label="Other stores">
                  {otherStores.map((s) => (
                    <option key={s.id} value={s.id}>
                      {s.name}
                    </option>
                  ))}
                </optgroup>
              )}
            </Select>
          </Field>

          <Field label="Bucket" hint={buckets.error ? <span className="text-warn">Couldn't list the buckets: {errorText(buckets.error)}. You can still name a new one.</span> : undefined}>
            <Select aria-label="Bucket" value={choice} disabled={!storeId || buckets.isLoading} onChange={(e) => set({ bucket: e.target.value })}>
              {!choice && <option value="">{!storeId ? "Choose a store first" : buckets.isLoading ? "Loading buckets…" : "Choose a bucket…"}</option>}
              {bucketList.map((b) => (
                <option key={b.name} value={b.name}>
                  {b.name}
                </option>
              ))}
              <option value={NEW}>New bucket…</option>
            </Select>
          </Field>
          {isNew && (
            <Field label="New bucket name" hint={nameError ? <span className="text-bad">{nameError}</span> : "Created in the store when you attach. 3–63 lowercase letters, digits, dots and hyphens."}>
              <Input autoFocus className="font-mono" maxLength={63} aria-invalid={!!nameError} value={form.new_name} onChange={(e) => set({ new_name: e.target.value.toLowerCase() })} />
            </Field>
          )}

          <div className="grid gap-3 sm:grid-cols-2">
            <Field label="Access">
              <Select aria-label="Access" value={form.access} onChange={(e) => set({ access: e.target.value === "read" ? "read" : "readwrite" })}>
                <option value="readwrite">{ACCESS_LABEL.readwrite}</option>
                <option value="read">{ACCESS_LABEL.read}</option>
              </Select>
            </Field>
            <Field label="Variable prefix" hint={prefixError ? <span className="text-bad">{prefixError}</span> : "Uppercase letters, digits and underscores."}>
              <Input className="font-mono" maxLength={PREFIX_MAX} aria-invalid={!!prefixError} value={form.prefix} onChange={(e) => set({ prefix: e.target.value.toUpperCase() })} />
            </Field>
          </div>

          <div className="rounded-md border border-line px-3 py-2">
            <div className="mb-1 text-[11px] font-semibold tracking-wide text-faint uppercase">Variables the service gets</div>
            <ul className="grid gap-x-4 sm:grid-cols-2">
              {names.map((n) => (
                <li key={n} className={`truncate font-mono text-[11px] ${defined.has(n) ? "text-bad" : ""}`} title={n}>
                  {n}
                </li>
              ))}
            </ul>
            <p className="mt-1.5 text-[11px] text-faint">Added with the next deploy. The secret stays in the vault.</p>
          </div>

          {attach.error && (
            <p role="alert" className="rounded-md bg-bad-soft px-3 py-2 text-xs break-words text-bad">
              {mutationText(attach.error)}
            </p>
          )}

          <div className="flex justify-end gap-2 pt-1">
            <Button type="button" variant="ghost" onClick={onClose} disabled={attach.isPending}>
              Cancel
            </Button>
            <Button variant="primary" disabled={attach.isPending || !!blocked} title={blocked}>
              {attach.isPending && <Loader2 className="size-4 animate-spin" />}
              Attach bucket
            </Button>
          </div>
        </form>
      )}
    </Modal>
  );
}
