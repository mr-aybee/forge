import { useQuery, useQueryClient } from "@tanstack/react-query";
import { KeyRound, Loader2, Pencil, ShieldOff, TriangleAlert } from "lucide-react";
import { useState } from "react";

import { type FileGrant, type GrantChanges, type IssuedGrant, type SftpInfo, resetGrantPassword, revokeGrant, sftpInfo, updateGrant, useFileRoots } from "../../api/files";
import { errorText, useToast } from "../../app/toast";
import { Badge, Button, Confirm, CopyValue, Field, Input, Modal, RowMenu, Select, Skeleton, Table, Td, bytes, time } from "../ui";

/** Expiry choices. "never" is offered only when the administrator allows it. */
export const EXPIRY = [
  { label: "1 hour", minutes: 60 },
  { label: "24 hours", minutes: 24 * 60 },
  { label: "7 days", minutes: 7 * 24 * 60 },
] as const;

/** Why "Never" is unavailable, or null when it may be chosen. */
export function neverBlocked(info: SftpInfo | undefined): string | null {
  if (!info?.allow_never_expiry) return "disabled by the administrator ([sftp] allow_never_expiry)";
  if (!info.can_never_expire) return "needs the file administrator role";
  return null;
}

/** Expiry select: presets within `max_hours`, Custom, and Never (disabled with the reason). */
export function ExpiryPicker({
  info,
  value,
  onChange,
  custom,
  onCustom,
  keepLabel,
}: {
  info: SftpInfo | undefined;
  value: string;
  onChange: (v: string) => void;
  custom: string;
  onCustom: (v: string) => void;
  /** Offer "Keep current expiry" (editing). */
  keepLabel?: string;
}) {
  const blocked = neverBlocked(info);
  const max = info?.max_hours ?? 168;
  return (
    <div className="grid gap-3 sm:grid-cols-2">
      <Field label="Expires after" hint={value === "never" ? undefined : blocked ? `Never: ${blocked}` : undefined}>
        <Select value={value} onChange={(e) => onChange(e.target.value)}>
          {keepLabel && <option value="keep">{keepLabel}</option>}
          {EXPIRY.filter((x) => x.minutes <= max * 60).map((x) => (
            <option key={x.minutes} value={String(x.minutes)}>
              {x.label}
            </option>
          ))}
          <option value="custom">Custom…</option>
          <option value="never" disabled={!!blocked}>
            {blocked ? `Never (${blocked})` : "Never"}
          </option>
        </Select>
      </Field>
      {value === "custom" && (
        <Field label="Hours" hint={`At most ${max}`}>
          <Input type="number" min={0.1} step={0.5} value={custom} onChange={(e) => onCustom(e.target.value)} />
        </Field>
      )}
      {value === "never" && (
        <p className="flex items-start gap-1.5 self-end text-xs text-warn sm:col-span-2">
          <TriangleAlert className="mt-0.5 size-3.5 shrink-0" />
          This access never ends on its own. Anyone with the password keeps access until you revoke it or its creator loses their role. Prefer an expiry, an address
          restriction and a transfer allowance.
        </p>
      )}
    </div>
  );
}

/** Minutes for a picker value, or null for "never"/"keep". */
export function pickedMinutes(value: string, custom: string): number | null {
  if (value === "never" || value === "keep") return null;
  return value === "custom" ? Math.round(Number(custom) * 60) : Number(value);
}

/** Connection details with a password shown once. */
export function ConnectionCard({ issued, title = "Copy the password now. Forge shows it only once." }: { issued: IssuedGrant; title?: string }) {
  const c = issued.connection;
  return (
    <div className="rounded-lg border border-warn bg-warn-soft p-3 text-xs">
      <p className="mb-2 font-medium text-warn">{title}</p>
      <dl className="grid grid-cols-[7rem_minmax(0,1fr)] items-center gap-x-3 gap-y-1">
        {(
          [
            ["Protocol", "SFTP"],
            ["Host", c.host],
            ["Port", String(c.port ?? "")],
            ["Username", c.username],
            ["Password", c.password],
            ["Expires", issued.grant.never_expires ? "never" : time(c.expires_at)],
            ["Host key", c.fingerprint],
          ] as const
        ).map(([k, v]) => (
          <div key={k} className="contents">
            <dt className="text-muted">{k}</dt>
            <dd className="min-w-0">
              {k === "Protocol" || k === "Expires" ? (
                <span className="block truncate font-mono" title={v}>
                  {v || "—"}
                </span>
              ) : (
                <CopyValue text={v ?? ""} label={k} />
              )}
            </dd>
          </div>
        ))}
      </dl>
      <p className="mt-2 text-muted">Use these in FileZilla, Cyberduck or WinSCP (SFTP). Check that the client shows the host key above on first connect.</p>
    </div>
  );
}

const statusTone = { active: "ok", expired: "muted", revoked: "bad" } as const;

function permissions(g: FileGrant) {
  return g.write ? (g.delete ? "read, upload, delete" : "read, upload") : "read-only";
}

/** Grants as a table with Edit, Reset password and Revoke. */
export function GrantList({ grants, showNode, rootLabel, onChanged }: { grants: FileGrant[]; showNode?: boolean; rootLabel?: (id: string) => string; onChanged: () => void }) {
  const toast = useToast();
  const [editing, setEditing] = useState<FileGrant | null>(null);
  const [revoking, setRevoking] = useState<FileGrant | null>(null);
  const [resetting, setResetting] = useState<FileGrant | null>(null);
  const [issued, setIssued] = useState<IssuedGrant | null>(null);
  const [busy, setBusy] = useState(false);
  const label = rootLabel ?? ((id: string) => id);

  const revoke = async (g: FileGrant) => {
    setBusy(true);
    try {
      await revokeGrant(g.server_id, g.id);
      toast("Access revoked; open sessions were closed", "ok");
      onChanged();
      setRevoking(null);
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(false);
    }
  };
  const reset = async (g: FileGrant) => {
    setBusy(true);
    try {
      setIssued(await resetGrantPassword(g.server_id, g.id));
      onChanged();
      setResetting(null);
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      {issued && (
        <div className="mb-3">
          <ConnectionCard issued={issued} title="New password for this access. Copy it now. Forge shows it only once." />
        </div>
      )}
      <Table head={["Name", ...(showNode ? ["Node"] : []), "Folders", "Permissions", "Created", "Expires", "Last used", "Transferred", "Status", ""]}>
        {grants.map((g) => (
          <tr key={g.id}>
            <Td>
              <div className="font-medium">{g.name}</div>
              <div className="font-mono text-[11px] text-faint">{g.id}</div>
            </Td>
            {showNode && <Td>{g.server_name ?? g.server_id}</Td>}
            <Td className="text-muted">{g.roots.map(label).join(", ")}</Td>
            <Td className="text-muted">{permissions(g)}</Td>
            <Td className="text-muted">
              <div>{time(g.created_at)}</div>
              <div className="text-[11px] text-faint">by {g.created_by}</div>
              {g.owner && g.owner !== g.created_by && <div className="text-[11px] text-faint">owner {g.owner}</div>}
            </Td>
            <Td>
              {g.never_expires ? (
                <Badge tone="warn">never expires</Badge>
              ) : (
                <span className="text-muted">{time(g.expires_at)}</span>
              )}
            </Td>
            <Td className="text-muted">{time(g.last_used_at)}</Td>
            <Td className="text-muted">
              {bytes(g.bytes_transferred)}
              {g.transfer_limit ? ` / ${bytes(g.transfer_limit)}` : ""}
            </Td>
            <Td>
              <Badge tone={statusTone[g.status] ?? "muted"}>{g.status}</Badge>
            </Td>
            <Td>
              <RowMenu
                actions={[
                  { label: "Edit", icon: Pencil, onSelect: () => setEditing(g), hidden: g.status === "revoked" },
                  { label: "Reset password", icon: KeyRound, onSelect: () => setResetting(g), hidden: g.status === "revoked" },
                  { label: "Revoke", icon: ShieldOff, onSelect: () => setRevoking(g), danger: true, hidden: g.status === "revoked" },
                ]}
              />
            </Td>
          </tr>
        ))}
      </Table>
      {editing && <GrantEditor grant={editing} onClose={() => setEditing(null)} onSaved={onChanged} />}
      <Confirm
        open={!!revoking}
        onOpenChange={(o) => !o && setRevoking(null)}
        title="Revoke SFTP access?"
        body={
          <>
            <b>{revoking?.name}</b> stops working now and its open sessions close. This cannot be undone.
          </>
        }
        confirmLabel="Revoke"
        busy={busy}
        onConfirm={() => revoking && void revoke(revoking)}
      />
      <Confirm
        open={!!resetting}
        onOpenChange={(o) => !o && setResetting(null)}
        title="Reset the password?"
        body={
          <>
            The current password of <b>{resetting?.name}</b> stops working and its open sessions close. Forge shows the new password once.
          </>
        }
        confirmLabel="Reset password"
        busy={busy}
        onConfirm={() => resetting && void reset(resetting)}
      />
    </>
  );
}

const splitIps = (s: string) =>
  s
    .split(/[\s,]+/)
    .map((x) => x.trim())
    .filter(Boolean);

/** Changes folders, permissions, restrictions, name and expiry; the password stays. */
export function GrantEditor({ grant, onClose, onSaved }: { grant: FileGrant; onClose: () => void; onSaved: () => void }) {
  const roots = useFileRoots(grant.server_id);
  const info = useQuery({ queryKey: ["sftp-info", grant.server_id], queryFn: () => sftpInfo(grant.server_id), retry: false });
  const qc = useQueryClient();
  const toast = useToast();
  const [name, setName] = useState(grant.name);
  const [picked, setPicked] = useState<string[]>(grant.roots);
  const [write, setWrite] = useState(grant.write);
  const [del, setDel] = useState(grant.delete);
  const [expiry, setExpiry] = useState(grant.never_expires ? "never" : "keep");
  const [custom, setCustom] = useState("");
  const [ips, setIps] = useState(grant.allowed_ips.join(", "));
  const [sessions, setSessions] = useState(String(grant.max_sessions));
  const [kbps, setKbps] = useState(grant.bandwidth_kbps ? String(grant.bandwidth_kbps) : "");
  const initialLimit = grant.transfer_limit ? String(Math.round(grant.transfer_limit / 1024 / 1024)) : "";
  const [limitMb, setLimitMb] = useState(initialLimit);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const list = roots.data?.roots ?? [];
  // Approved roots the caller sees now, plus any the grant already has.
  const options = [...list.map((r) => ({ id: r.id, label: r.label, ro: r.read_only })), ...grant.roots.filter((id) => !list.some((r) => r.id === id)).map((id) => ({ id, label: id, ro: false }))];
  const canWrite = picked.every((id) => {
    const r = list.find((x) => x.id === id);
    return !r || (!r.read_only && r.permissions.write);
  });
  const canDelete = picked.every((id) => {
    const r = list.find((x) => x.id === id);
    return !r || r.permissions.delete;
  });
  const minutes = pickedMinutes(expiry, custom);

  const save = async () => {
    setBusy(true);
    setError(null);
    const c: GrantChanges = {};
    if (name.trim() !== grant.name) c.name = name.trim();
    if ([...picked].sort().join() !== [...grant.roots].sort().join()) c.roots = picked;
    const w = write && canWrite;
    const d = del && canDelete && w;
    if (w !== grant.write) c.write = w;
    if (d !== grant.delete) c.delete = d;
    if (expiry === "never" && !grant.never_expires) c.never_expires = true;
    if (minutes !== null) {
      c.expires_in_minutes = minutes;
      c.never_expires = false;
    }
    const newIps = splitIps(ips);
    if (newIps.join() !== grant.allowed_ips.join()) c.allowed_ips = newIps;
    const s = Number(sessions) || 1;
    if (s !== grant.max_sessions) c.max_sessions = s;
    const k = kbps ? Number(kbps) : null;
    if (k !== (grant.bandwidth_kbps ?? null)) c.bandwidth_kbps = k;
    if (limitMb !== initialLimit) c.transfer_limit = limitMb ? Math.round(Number(limitMb) * 1024 * 1024) : null;
    if (Object.keys(c).length === 0) {
      onClose();
      return;
    }
    try {
      await updateGrant(grant.server_id, grant.id, c);
      toast("Access updated; the password is unchanged", "ok");
      void qc.invalidateQueries({ queryKey: ["file-grants"] });
      void qc.invalidateQueries({ queryKey: ["all-file-grants"] });
      onSaved();
      onClose();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal open onOpenChange={(o) => !o && onClose()} title={`Edit ${grant.name}`} description="The password stays the same. Open sessions close when you take access away. Adding folders, upload or delete makes you the owner: the access then acts with your rights." wide>
      <form
        className="grid gap-3"
        onSubmit={(e) => {
          e.preventDefault();
          void save();
        }}
      >
        <Field label="Name">
          <Input value={name} onChange={(e) => setName(e.target.value)} />
        </Field>
        <Field label="Folders" hint="Only approved folders you can open">
          {roots.isLoading ? (
            <Skeleton rows={1} />
          ) : (
            <div className="flex flex-wrap gap-3 text-xs">
              {options.map((r) => (
                <label key={r.id} className="flex items-center gap-1.5">
                  <input type="checkbox" checked={picked.includes(r.id)} onChange={(e) => setPicked((p) => (e.target.checked ? [...p, r.id] : p.filter((x) => x !== r.id)))} />
                  {r.label}
                  {r.ro && <Badge tone="muted">read-only</Badge>}
                </label>
              ))}
            </div>
          )}
        </Field>
        <div className="flex flex-wrap gap-4 text-xs text-muted">
          <span>Read</span>
          <label className="flex items-center gap-1.5">
            <input type="checkbox" disabled={!canWrite} checked={write && canWrite} onChange={(e) => setWrite(e.target.checked)} /> Upload and create
          </label>
          <label className="flex items-center gap-1.5">
            <input type="checkbox" disabled={!canDelete || !write} checked={del && canDelete && write} onChange={(e) => setDel(e.target.checked)} /> Delete and replace
          </label>
        </div>
        <ExpiryPicker
          info={info.data}
          value={expiry}
          onChange={setExpiry}
          custom={custom}
          onCustom={setCustom}
          keepLabel={grant.never_expires ? undefined : `Keep (${time(grant.expires_at)})`}
        />
        <div className="grid gap-3 sm:grid-cols-2">
          <Field label="Allowed addresses" hint="IPs or CIDR ranges; empty for any">
            <Input className="font-mono" value={ips} onChange={(e) => setIps(e.target.value)} placeholder="203.0.113.0/24" />
          </Field>
          <Field label="Sessions at once">
            <Input type="number" min={1} max={10} value={sessions} onChange={(e) => setSessions(e.target.value)} />
          </Field>
          <Field label="Bandwidth per session (KiB/s)">
            <Input type="number" min={1} value={kbps} onChange={(e) => setKbps(e.target.value)} placeholder="unlimited" />
          </Field>
          <Field label="Transfer allowance (MiB)">
            <Input type="number" min={1} value={limitMb} onChange={(e) => setLimitMb(e.target.value)} placeholder="unlimited" />
          </Field>
        </div>
        {error && <p className="text-xs text-bad">{error}</p>}
        <div className="flex justify-end gap-2">
          <Button type="button" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" type="submit" disabled={busy || !name.trim() || picked.length === 0 || (minutes !== null && !(minutes > 0))}>
            {busy && <Loader2 className="size-4 animate-spin" />} Save changes
          </Button>
        </div>
      </form>
    </Modal>
  );
}
