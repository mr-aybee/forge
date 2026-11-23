import { useQuery } from "@tanstack/react-query";
import { KeyRound, Loader2, ShieldOff } from "lucide-react";
import { useState } from "react";

import { type FileRoot, type IssuedGrant, createGrant, listGrants, sftpInfo } from "../../api/files";
import { errorText } from "../../app/toast";
import { Badge, Button, Drawer, Empty, ErrorNote, Field, Input, Skeleton } from "../ui";
import { ConnectionCard, ExpiryPicker, GrantList, pickedMinutes } from "./GrantList";

/** Temporary SFTP access: issue (shown once), list, change, reset, revoke. */
export function AccessPanel({ serverId, roots, current, onClose }: { serverId: string; roots: FileRoot[]; current: FileRoot; onClose: () => void }) {
  const info = useQuery({ queryKey: ["sftp-info", serverId], queryFn: () => sftpInfo(serverId), retry: false });
  const grants = useQuery({ queryKey: ["file-grants", serverId], queryFn: () => listGrants(serverId), retry: false, enabled: !!info.data?.enabled });
  const [name, setName] = useState("");
  const [picked, setPicked] = useState<string[]>([current.id]);
  const [write, setWrite] = useState(false);
  const [del, setDel] = useState(false);
  const [expiry, setExpiry] = useState<string>("60");
  const [custom, setCustom] = useState("");
  const [ips, setIps] = useState("");
  const [sessions, setSessions] = useState("1");
  const [kbps, setKbps] = useState("");
  const [limitMb, setLimitMb] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [issued, setIssued] = useState<IssuedGrant | null>(null);

  const label = (id: string) => roots.find((r) => r.id === id)?.label ?? id;
  const never = expiry === "never";
  const minutes = pickedMinutes(expiry, custom) ?? 0;
  const canWrite = picked.every((id) => {
    const r = roots.find((x) => x.id === id);
    return r && !r.read_only && r.permissions.write;
  });
  const canDelete = picked.every((id) => roots.find((x) => x.id === id)?.permissions.delete);

  const issue = async () => {
    setBusy(true);
    setError(null);
    try {
      const r = await createGrant(serverId, {
        name: name.trim(),
        roots: picked,
        write: write && canWrite,
        delete: del && canDelete,
        expires_in_minutes: minutes,
        never_expires: never,
        allowed_ips: ips
          .split(/[\s,]+/)
          .map((s) => s.trim())
          .filter(Boolean),
        max_sessions: Number(sessions) || 1,
        bandwidth_kbps: kbps ? Number(kbps) : undefined,
        transfer_limit: limitMb ? Math.round(Number(limitMb) * 1024 * 1024) : undefined,
      });
      setIssued(r);
      setName("");
      void grants.refetch();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Drawer open onOpenChange={(o) => !o && onClose()} title="Temporary SFTP access" wide>
      {info.isLoading ? (
        <Skeleton rows={4} />
      ) : info.error ? (
        <ErrorNote error={info.error} />
      ) : !info.data?.enabled ? (
        <Empty icon={<ShieldOff className="size-5" />} title="The SFTP gateway is off">
          An administrator can turn it on with the <code>[sftp]</code> section of the Forge configuration.
        </Empty>
      ) : (
        <div className="grid gap-5">
          {issued && <ConnectionCard issued={issued} />}

          <form
            className="grid gap-3 rounded-lg border border-line p-3"
            onSubmit={(e) => {
              e.preventDefault();
              void issue();
            }}
          >
            <p className="text-xs text-muted">
              Clients connect to <span className="font-mono">{info.data.host}:{info.data.port}</span>. Access ends at expiry, when you revoke it, or when your own
              access ends.
            </p>
            <Field label="Name" hint="What it is for">
              <Input value={name} onChange={(e) => setName(e.target.value)} placeholder="Vendor attachment upload" />
            </Field>
            <Field label="Folders">
              <div className="flex flex-wrap gap-3 text-xs">
                {roots.map((r) => (
                  <label key={r.id} className="flex items-center gap-1.5">
                    <input type="checkbox" checked={picked.includes(r.id)} onChange={(e) => setPicked((p) => (e.target.checked ? [...p, r.id] : p.filter((x) => x !== r.id)))} />
                    {r.label}
                    {r.read_only && <Badge tone="muted">read-only</Badge>}
                  </label>
                ))}
              </div>
            </Field>
            <div className="flex flex-wrap gap-4 text-xs text-muted">
              <label className="flex items-center gap-1.5">
                <input type="checkbox" disabled={!canWrite} checked={write && canWrite} onChange={(e) => setWrite(e.target.checked)} /> Upload and create
              </label>
              <label className="flex items-center gap-1.5">
                <input type="checkbox" disabled={!canDelete || !write} checked={del && canDelete && write} onChange={(e) => setDel(e.target.checked)} /> Delete and
                replace
              </label>
            </div>
            <ExpiryPicker info={info.data} value={expiry} onChange={setExpiry} custom={custom} onCustom={setCustom} />
            <details className="text-xs">
              <summary className="cursor-pointer text-muted">Restrictions</summary>
              <div className="mt-3 grid gap-3 sm:grid-cols-2">
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
            </details>
            {error && <p className="text-xs text-bad">{error}</p>}
            <div className="flex justify-end">
              <Button variant="primary" type="submit" disabled={busy || !name.trim() || picked.length === 0 || (!never && !(minutes > 0))}>
                {busy ? <Loader2 className="size-4 animate-spin" /> : <KeyRound className="size-4" />} Create SFTP access
              </Button>
            </div>
          </form>

          <div>
            <h3 className="mb-2 text-sm font-medium">Temporary access</h3>
            <p className="mb-2 text-xs text-faint">
              Every node's access is listed under <b>Administration → Access → SFTP access</b>.
            </p>
            {grants.isLoading ? (
              <Skeleton rows={3} />
            ) : grants.error ? (
              <ErrorNote error={grants.error} />
            ) : !grants.data?.length ? (
              <p className="text-xs text-faint">None issued on this node.</p>
            ) : (
              <GrantList grants={grants.data} rootLabel={label} onChanged={() => void grants.refetch()} />
            )}
          </div>
        </div>
      )}
    </Drawer>
  );
}
