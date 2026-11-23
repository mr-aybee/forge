import { Loader2, Plus, RefreshCw, ShieldCheck, Trash2, UserCheck } from "lucide-react";
import { useState } from "react";

import { ApiError } from "../../api/client";
import { useMe, useUsers } from "../../api/hooks";
import { useBuckets, useCreateBucketGrant, useRevokeBucketGrant, useStoreGrants } from "../../api/storage";
import type { BucketGrant, KeyAccess, ObjectStore } from "../../api/types";
import { loginOf } from "../../api/users";
import { errorText, useToast } from "../../app/toast";
import { Button, Confirm, Empty, ErrorNote, Field, IconButton, Input, Modal, Panel, RowMenu, type RowAction, Select, Skeleton, Status, Table, Td, ago, time, useRowMenu } from "../ui";

const ACCESS: { id: KeyAccess; label: string; option: string }[] = [
  { id: "read", label: "Read", option: "Read: list and download" },
  { id: "readwrite", label: "Read and write", option: "Read and write: also upload and delete" },
];
export const grantAccessLabel = (a: KeyAccess) => ACCESS.find((x) => x.id === a)?.label ?? a;

const DATE = "date";
const EXPIRY: { id: string; label: string; minutes: number | null }[] = [
  { id: "never", label: "Never", minutes: null },
  { id: "1d", label: "1 day", minutes: 24 * 60 },
  { id: "7d", label: "7 days", minutes: 7 * 24 * 60 },
  { id: "30d", label: "30 days", minutes: 30 * 24 * 60 },
  { id: DATE, label: "On a date…", minutes: null },
];

/**
 * Bucket grants of a store: who may open which bucket through Forge without
 * a role on the store. Shown to people who manage the store; the list needs
 * `storage.manage` and an MFA-verified sign-in.
 */
export function BucketGrants({ s }: { s: ObjectStore }) {
  const me = useMe();
  const [asked, setAsked] = useState(false);
  const [all, setAll] = useState(false);
  const [creating, setCreating] = useState(false);
  const [revoking, setRevoking] = useState<BucketGrant | null>(null);
  // Reading the list asks an unverified session to step up. Wait for the
  // person to ask for it instead of prompting everyone who opens the page.
  const verified = me.data?.mfa === true;
  const grants = useStoreGrants(s.id, all, verified || asked);
  const revoke = useRevokeBucketGrant();
  const toast = useToast();
  const rm = useRowMenu();
  const list = grants.data ?? [];

  const refused = grants.error instanceof ApiError && grants.error.status === 403 && grants.error.code !== "step_up_required";
  // Managing access is not theirs to do: the section is for the people who
  // manage the store. Someone who asked for the list is told why instead.
  if (refused && !asked) return null;

  const actions = (g: BucketGrant): RowAction[] =>
    g.status === "active" ? [{ label: "Revoke", icon: Trash2, danger: true, onSelect: () => setRevoking(g), disabled: revoke.isPending, hint: revoke.isPending ? "A revocation is running" : undefined }] : [];
  const loaded = verified || asked;

  return (
    <Panel
      title="Access"
      description="People who can open one bucket of this store through Forge without a role on it. A grant does not issue an S3 key."
      actions={
        loaded && !grants.error ? (
          <div className="flex flex-wrap items-center gap-2">
            <label className="flex items-center gap-1.5 text-xs text-muted">
              <input type="checkbox" checked={all} onChange={(e) => setAll(e.target.checked)} />
              Show expired and revoked
            </label>
            <IconButton label="Refresh access" onClick={() => void grants.refetch()}>
              <RefreshCw className={`size-3.5 ${grants.isFetching ? "animate-spin" : ""}`} />
            </IconButton>
            <Button variant="primary" title="Let one person open one bucket" onClick={() => setCreating(true)}>
              <Plus className="size-4" /> Grant access
            </Button>
          </div>
        ) : undefined
      }
    >
      {!loaded ? (
        <div className="flex flex-wrap items-center justify-between gap-3 px-4 py-3 text-xs text-muted">
          <span>Seeing and changing who has access needs the storage.manage permission and an MFA-verified sign-in.</span>
          <Button onClick={() => setAsked(true)}>
            <ShieldCheck className="size-4" /> Show access
          </Button>
        </div>
      ) : grants.isLoading ? (
        <Skeleton rows={2} />
      ) : grants.error ? (
        <ErrorNote error={grants.error} />
      ) : !list.length ? (
        <Empty icon={<UserCheck className="size-5" />} title={all ? "No grants" : "No one has a grant"} action={<Button onClick={() => setCreating(true)}><Plus className="size-4" /> Grant access</Button>}>
          Give one person access to one bucket without giving them a role on the store.
        </Empty>
      ) : (
        <Table head={["Person", "Bucket", "Access", "Expires", "Granted by", "Note", "Status", ""]}>
          {list.map((g) => (
            <tr key={g.id} className={`text-[13px] hover:bg-panel-2/50 ${g.status === "active" ? "" : "opacity-60"}`} {...rm.rowProps(actions(g), `Actions for the grant of ${g.user}`)}>
              <Td className="max-w-[14rem]">
                <span className="block truncate font-medium" title={g.user}>
                  {g.user}
                </span>
              </Td>
              <Td className="max-w-[12rem]">
                <span className="block truncate font-mono text-[11px]" title={g.bucket}>
                  {g.bucket}
                </span>
              </Td>
              <Td className="text-xs whitespace-nowrap">{grantAccessLabel(g.access)}</Td>
              <Td className="text-xs whitespace-nowrap text-muted" title={g.expires_at ? time(g.expires_at) : "This grant has no end"}>
                {g.expires_at ? ago(g.expires_at) : "Never"}
              </Td>
              <Td className="max-w-[12rem] text-xs text-muted">
                <span className="block truncate" title={`${g.created_by_name ?? g.created_by} · ${time(g.created_at)}`}>
                  {g.created_by_name ?? "—"}
                </span>
              </Td>
              <Td className="max-w-[16rem] text-xs text-muted">
                <span className="block truncate" title={g.note || undefined}>
                  {g.note || "—"}
                </span>
              </Td>
              <Td title={g.revoked_at ? `Revoked ${time(g.revoked_at)}` : undefined}>
                <Status value={g.status} />
              </Td>
              <Td className="w-10 text-right">
                <RowMenu label={`Actions for the grant of ${g.user}`} actions={actions(g)} />
              </Td>
            </tr>
          ))}
        </Table>
      )}
      {rm.menu}

      {creating && <GrantDialog s={s} onClose={() => setCreating(false)} />}
      <Confirm
        open={!!revoking}
        onOpenChange={(o) => !o && setRevoking(null)}
        title={`Revoke access for ${revoking?.user ?? ""}?`}
        body={
          <p>
            {revoking?.user} loses {revoking ? grantAccessLabel(revoking.access).toLowerCase() : ""} access to bucket <span className="font-mono text-xs text-fg">{revoking?.bucket}</span> with their next request. Roles and other grants are not changed.
          </p>
        }
        confirmLabel="Revoke"
        busy={revoke.isPending}
        onConfirm={() => {
          const g = revoking;
          if (!g) return;
          revoke.mutate(
            { id: s.id, grantId: g.id },
            {
              onSuccess: () => {
                setRevoking(null);
                toast(`Revoked access for ${g.user}`);
              },
              onError: (e) => toast(errorText(e), "error"),
            },
          );
        }}
      />
    </Panel>
  );
}

interface GrantForm {
  user: string;
  bucket: string;
  access: KeyAccess;
  expiry: string;
  date: string;
  note: string;
}

/** Grant one person access to one bucket. Mount it while open: the initial state is the reset. */
function GrantDialog({ s, onClose }: { s: ObjectStore; onClose: () => void }) {
  const users = useUsers();
  const buckets = useBuckets(s.id);
  const create = useCreateBucketGrant();
  const toast = useToast();
  const [form, setForm] = useState<GrantForm>({ user: "", bucket: "", access: "read", expiry: "never", date: "", note: "" });
  const set = (patch: Partial<GrantForm>) => setForm((f) => ({ ...f, ...patch }));
  const people = (users.data ?? []).filter((u) => !u.disabled);
  const names = buckets.data ?? [];
  // The only bucket is the one meant.
  const bucket = form.bucket || (names.length === 1 ? (names[0]?.name ?? "") : "");
  const until = form.expiry === DATE && form.date ? new Date(form.date).getTime() : NaN;
  const dateOk = form.expiry !== DATE || (Number.isFinite(until) && until > Date.now());
  const valid = !!form.user && !!bucket && dateOk;

  const submit = () => {
    const minutes = EXPIRY.find((x) => x.id === form.expiry)?.minutes;
    const person = people.find((u) => u.id === form.user);
    create.mutate(
      {
        id: s.id,
        user: form.user,
        bucket,
        access: form.access,
        ...(form.expiry === DATE ? { expires_at: until } : minutes ? { lifetime_minutes: minutes } : {}),
        ...(form.note.trim() ? { note: form.note.trim() } : {}),
      },
      {
        onSuccess: (g) => {
          toast(`Granted ${g.user || (person ? loginOf(person) : "the person")} access to ${g.bucket || bucket}`);
          onClose();
        },
        onError: (e) => toast(errorText(e), "error"),
      },
    );
  };

  return (
    <Modal open onOpenChange={(o) => !o && onClose()} title="Grant access" description={`Lets one person open one bucket of ${s.name} through Forge: the console, the API and the CLI. No S3 key is issued.`}>
      <form
        className="space-y-3"
        onSubmit={(e) => {
          e.preventDefault();
          if (valid) submit();
        }}
      >
        {users.error ? <ErrorNote error={users.error} /> : null}
        {buckets.error ? <ErrorNote error={buckets.error} /> : null}
        <Field label="Person">
          <Select value={form.user} onChange={(e) => set({ user: e.target.value })} required aria-label="Person">
            <option value="">Choose a person…</option>
            {people.map((u) => (
              <option key={u.id} value={u.id}>
                {u.display_name ? `${u.display_name} (${loginOf(u)})` : loginOf(u)}
              </option>
            ))}
          </Select>
        </Field>
        <div className="grid gap-3 sm:grid-cols-2">
          <Field label="Bucket" hint={!buckets.isLoading && !buckets.error && !names.length ? "This store has no buckets yet. Create one first." : undefined}>
            <Select value={bucket} onChange={(e) => set({ bucket: e.target.value })} required aria-label="Bucket">
              <option value="">Choose a bucket…</option>
              {names.map((b) => (
                <option key={b.name} value={b.name}>
                  {b.name}
                </option>
              ))}
            </Select>
          </Field>
          <Field label="Access">
            <Select value={form.access} onChange={(e) => set({ access: e.target.value === "readwrite" ? "readwrite" : "read" })} aria-label="Access">
              {ACCESS.map((a) => (
                <option key={a.id} value={a.id}>
                  {a.option}
                </option>
              ))}
            </Select>
          </Field>
        </div>
        <div className="grid gap-3 sm:grid-cols-2">
          <Field label="Expires">
            <Select value={form.expiry} onChange={(e) => set({ expiry: e.target.value })} aria-label="Expires">
              {EXPIRY.map((x) => (
                <option key={x.id} value={x.id}>
                  {x.label}
                </option>
              ))}
            </Select>
          </Field>
          {form.expiry === DATE && (
            <Field label="Expires on" hint={form.date && !dateOk ? "Choose a time in the future." : undefined}>
              <Input type="datetime-local" required value={form.date} onChange={(e) => set({ date: e.target.value })} />
            </Field>
          )}
        </div>
        <Field label="Note" hint="Optional. Shown in this list and in the audit log.">
          <Input maxLength={200} value={form.note} onChange={(e) => set({ note: e.target.value })} />
        </Field>
        {form.access === "readwrite" && <p className="text-xs text-muted">Read and write lets this person delete objects in the bucket.</p>}
        <div className="flex justify-end gap-2">
          <Button type="button" variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" disabled={!valid || create.isPending}>
            {create.isPending && <Loader2 className="size-4 animate-spin" />}
            Grant access
          </Button>
        </div>
      </form>
    </Modal>
  );
}
