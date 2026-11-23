import { Loader2 } from "lucide-react";
import { useMemo, useState } from "react";

import { useServices, useCatalog, useCreateTunnelGrant, useInstallations, useServers, useUsers, type TunnelGrants } from "../../api/hooks";
import { useStacks } from "../../api/stacks";
import { nodeName } from "../../api/types";
import { loginOf } from "../../api/users";
import { errorText, useToast } from "../../app/toast";
import { Button, Field, Input, Modal, Select } from "../ui";
import { TARGET_KINDS } from "./parts";

const LIFETIMES: { id: string; label: string; minutes: number | null }[] = [
  { id: "30m", label: "30 minutes", minutes: 30 },
  { id: "1h", label: "1 hour", minutes: 60 },
  { id: "8h", label: "8 hours", minutes: 8 * 60 },
  { id: "1d", label: "1 day", minutes: 24 * 60 },
  { id: "7d", label: "7 days", minutes: 7 * 24 * 60 },
  { id: "never", label: "Never expires", minutes: null },
];

const PORTS_RE = /^\s*\d{1,5}(\s*,\s*\d{1,5})*\s*$/;

/**
 * Create grant: who, which target, how long. Raw ports are offered to
 * administrators only; "never" only when the server allows it for the caller.
 */
export function CreateGrantDialog({ open, onOpenChange, grants, admin }: { open: boolean; onOpenChange: (o: boolean) => void; grants: TunnelGrants | undefined; admin: boolean }) {
  const users = useUsers().data ?? [];
  const nodes = useServers().data ?? [];
  const apps = useServices(undefined, undefined, open).data ?? [];
  const stacks = useStacks({}, open).data ?? [];
  const catalog = useCatalog().data?.packages ?? [];
  const installs = useInstallations().data ?? [];
  const create = useCreateTunnelGrant();
  const toast = useToast();

  const [user, setUser] = useState("");
  const [kind, setKind] = useState("app");
  const [app, setApp] = useState("");
  const [stack, setStack] = useState("");
  const [service, setService] = useState("");
  const [pkg, setPkg] = useState("");
  const [node, setNode] = useState("");
  const [ports, setPorts] = useState("");
  const [lifetime, setLifetime] = useState("8h");
  const [note, setNote] = useState("");

  const dbPackages = useMemo(() => {
    const installed = new Set(installs.map((i) => i.package));
    return catalog.filter((p) => p.category === "databases" || (installed.has(p.id) && p.ports.length > 0));
  }, [catalog, installs]);
  const stackEntry = stacks.find((s) => s.stack.id === stack);
  const services = [...new Set((stackEntry?.nodes ?? []).flatMap((n) => n.services ?? []))].sort();
  const appNames = [...new Map(apps.map((a) => [a.name, a])).values()].sort((a, b) => a.name.localeCompare(b.name));
  const neverOk = !!grants?.allow_never_expiry && !!grants?.can_never_expire;
  const kinds = Object.entries(TARGET_KINDS).filter(([k]) => k !== "port" || admin);

  const ref = kind === "app" ? app : kind === "stack" ? (stack && service ? `${stackEntry?.stack.name ?? stack}/${service}` : "") : kind === "package" ? pkg : node;
  const needsNode = kind === "ssh" || kind === "port";
  const portsOk = kind !== "port" || PORTS_RE.test(ports);
  const valid = !!user && !!ref && (!needsNode || !!node) && portsOk && (lifetime !== "never" || neverOk);

  const submit = async () => {
    const lt = LIFETIMES.find((l) => l.id === lifetime);
    try {
      await create.mutateAsync({
        user_id: user,
        target: { kind, ref, ...(node ? { server_id: node } : {}), ...(kind === "port" ? { ports: ports.replace(/\s+/g, "") } : {}) },
        ...(lt?.minutes ? { lifetime_minutes: lt.minutes } : { never_expires: true }),
        ...(note.trim() ? { note: note.trim() } : {}),
      });
      toast("Grant created");
      onOpenChange(false);
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  return (
    <Modal open={open} onOpenChange={onOpenChange} title="Create tunnel grant" description="Lets one person open tunnels to one target without a role on its environment.">
      <form
        className="space-y-3"
        onSubmit={(e) => {
          e.preventDefault();
          if (valid) void submit();
        }}
      >
        <Field label="User">
          <Select value={user} onChange={(e) => setUser(e.target.value)} required aria-label="User">
            <option value="">Choose a user…</option>
            {users
              .filter((u) => !u.disabled)
              .map((u) => (
                <option key={u.id} value={u.id}>
                  {u.display_name ? `${u.display_name} (${loginOf(u)})` : loginOf(u)}
                </option>
              ))}
          </Select>
        </Field>
        <div className="grid gap-3 sm:grid-cols-2">
          <Field label="Target">
            <Select
              value={kind}
              onChange={(e) => {
                setKind(e.target.value);
                setNode("");
              }}
              aria-label="Target kind"
            >
              {kinds.map(([k, label]) => (
                <option key={k} value={k}>
                  {label}
                </option>
              ))}
            </Select>
          </Field>
          {kind === "app" && (
            <Field label="Service">
              <Select value={app} onChange={(e) => setApp(e.target.value)} required aria-label="Service">
                <option value="">Choose…</option>
                {appNames.map((a) => (
                  <option key={a.id} value={a.name}>
                    {a.spec?.environment ? `${a.spec.environment}/${a.name}` : a.name}
                  </option>
                ))}
              </Select>
            </Field>
          )}
          {kind === "stack" && (
            <Field label="Stack">
              <Select
                value={stack}
                onChange={(e) => {
                  setStack(e.target.value);
                  setService("");
                }}
                required
                aria-label="Stack"
              >
                <option value="">Choose…</option>
                {stacks.map((s) => (
                  <option key={s.stack.id} value={s.stack.id}>
                    {s.stack.name}
                  </option>
                ))}
              </Select>
            </Field>
          )}
          {kind === "package" && (
            <Field label="Database package">
              <Select value={pkg} onChange={(e) => setPkg(e.target.value)} required aria-label="Database package">
                <option value="">Choose…</option>
                {dbPackages.map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.name}
                  </option>
                ))}
              </Select>
            </Field>
          )}
          {needsNode && (
            <Field label="Node">
              <Select value={node} onChange={(e) => setNode(e.target.value)} required aria-label="Node">
                <option value="">Choose…</option>
                {nodes.map((n) => (
                  <option key={n.id} value={n.id}>
                    {nodeName(n)}
                  </option>
                ))}
              </Select>
            </Field>
          )}
        </div>
        {kind === "stack" && (
          <Field label="Service" hint={stack && !services.length ? "This stack has not been deployed yet; deploy it so its services are known." : undefined}>
            <Select value={service} onChange={(e) => setService(e.target.value)} required disabled={!stack} aria-label="Service">
              <option value="">Choose…</option>
              {services.map((s) => (
                <option key={s} value={s}>
                  {s}
                </option>
              ))}
            </Select>
          </Field>
        )}
        {kind === "port" && (
          <Field label="Ports" hint={portsOk || !ports ? "Comma-separated TCP ports on the node. Raw ports bypass the service catalogue; every open is audited as raw." : "Ports are numbers, separated by commas."}>
            <Input value={ports} onChange={(e) => setPorts(e.target.value)} placeholder="9000, 9443" required aria-invalid={!portsOk && !!ports} />
          </Field>
        )}
        {(kind === "app" || kind === "stack" || kind === "package") && (
          <Field label="Node" hint="Leave empty to cover every node the service runs on.">
            <Select value={node} onChange={(e) => setNode(e.target.value)} aria-label="Node (optional)">
              <option value="">Any node</option>
              {nodes.map((n) => (
                <option key={n.id} value={n.id}>
                  {nodeName(n)}
                </option>
              ))}
            </Select>
          </Field>
        )}
        <div className="grid gap-3 sm:grid-cols-2">
          <Field
            label="Lifetime"
            hint={
              lifetime === "never" && !neverOk
                ? !grants?.allow_never_expiry
                  ? "Never-expiring grants are off: set [tunnels] allow_never_expiry on the server."
                  : "Only administrators may issue never-expiring grants."
                : "Tunnels opened under this grant end when it expires."
            }
          >
            <Select value={lifetime} onChange={(e) => setLifetime(e.target.value)} aria-label="Lifetime">
              {LIFETIMES.map((l) => (
                <option key={l.id} value={l.id} disabled={l.id === "never" && !neverOk}>
                  {l.label}
                  {l.id === "never" && !neverOk ? " (not allowed)" : ""}
                </option>
              ))}
            </Select>
          </Field>
          <Field label="Note (optional)">
            <Input value={note} onChange={(e) => setNote(e.target.value)} placeholder="Ticket, reason…" maxLength={200} />
          </Field>
        </div>
        <div className="flex justify-end gap-2 pt-1">
          <Button type="button" variant="ghost" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button variant="primary" disabled={!valid || create.isPending} title={valid ? undefined : "Pick a user and a complete target"}>
            {create.isPending && <Loader2 className="size-4 animate-spin" />}
            Create grant
          </Button>
        </div>
      </form>
    </Modal>
  );
}
