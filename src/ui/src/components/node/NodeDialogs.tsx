import { AlertTriangle, ShieldCheck } from "lucide-react";
import { useEffect, useState } from "react";

import { ApiError } from "../../api/client";
import { useCapabilities, useMe, useRemoveServer, useRenameServer, useRevokeServer, useServer, useSetDnsName } from "../../api/hooks";
import { nodeName, type Server } from "../../api/types";
import { errorText, useToast } from "../../app/toast";
import { Button, Field, Input, Modal } from "../ui";

/**
 * Revoking and removing are permanent, so both ask for the node's hostname
 * and, when the user has an authenticator, a fresh code. The server enforces
 * both. `remove` also deletes the node and its data; `onRemoved` runs after.
 */
export function RevokeNodeDialog({
  node,
  open,
  onOpenChange,
  mode = "revoke",
  onRemoved,
}: {
  node: Server;
  open: boolean;
  onOpenChange(o: boolean): void;
  mode?: "revoke" | "remove";
  onRemoved?(): void;
}) {
  const me = useMe().data;
  const revokeAction = useRevokeServer();
  const removeAction = useRemoveServer();
  const remove = mode === "remove";
  const revoke = remove ? removeAction : revokeAction;
  const verb = remove ? "Remove" : "Revoke";
  const toast = useToast();
  const [typed, setTyped] = useState("");
  const [code, setCode] = useState("");
  const [error, setError] = useState<string>();
  useEffect(() => {
    if (open) {
      setTyped("");
      setCode("");
      setError(undefined);
    }
  }, [open]);
  const caps = useCapabilities().data?.capabilities ?? [];
  const mfaCap = caps.find((c) => c.id === "security.mfa_required");
  const devNoMfa = mfaCap ? !mfaCap.enabled : caps.some((c) => c.id === "dev.skip_mfa" && c.enabled);
  const needsCode = (me?.mfa_enrolled ?? false) && !devNoMfa;
  const matches = typed.trim() === node.hostname;
  const ready = matches && (!needsCode || /^\d{6}$/.test(code.replace(/\s/g, "")));

  return (
    <Modal open={open} onOpenChange={onOpenChange} title={`${verb} ${nodeName(node)}?`}>
      <form
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault();
          if (!ready) return;
          setError(undefined);
          revoke.mutate(
            { id: node.id, confirm_hostname: typed.trim(), totp_code: needsCode ? code.replace(/\s/g, "") : undefined },
            {
              onSuccess: () => {
                toast(`${nodeName(node)} ${remove ? "removed" : "revoked"}`);
                onOpenChange(false);
                if (remove) onRemoved?.();
              },
              onError: (err) => setError(err instanceof ApiError ? err.message : errorText(err)),
            },
          );
        }}
      >
        <div className="flex gap-3 rounded-lg border border-bad/30 bg-bad-soft p-3 text-[13px] text-fg">
          <AlertTriangle className="mt-0.5 size-5 shrink-0 text-bad" />
          {remove ? (
            <ul className="list-disc space-y-1 pl-4">
              <li>The agent's certificate is revoked and the node is deleted from Forge.</li>
              <li>
                Its services, stack parts, deploy slots, file roots and grants, tunnels, logins, metrics history, alarms and tags are
                deleted. A stack that used it as default node moves to another of its nodes, or is deleted if it has none.
              </li>
              <li>Nothing on the machine itself is touched. The audit log keeps the record.</li>
              <li>This cannot be undone. The machine can enroll again under the same name as a fresh node.</li>
            </ul>
          ) : (
            <ul className="list-disc space-y-1 pl-4">
              <li>The agent's certificate is revoked immediately; it is cut off on its next request.</li>
              <li>Open terminals, running jobs and live metrics for this node stop.</li>
              <li>Bringing it back needs a new enrollment token and a reinstall of the agent.</li>
            </ul>
          )}
        </div>
        <Field label={`Type the node's hostname to confirm`} hint={<span className="font-mono">{node.hostname}</span>}>
          <Input value={typed} onChange={(e) => setTyped(e.target.value)} autoComplete="off" spellCheck={false} placeholder={node.hostname} autoFocus />
        </Field>
        {needsCode && (
          <Field label="Authenticator code" hint={`${remove ? "Removing" : "Revoking"} a node needs a fresh code even in a verified session.`}>
            <div className="relative">
              <ShieldCheck className="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-faint" />
              <Input
                className="pl-8 font-mono tracking-[0.25em]"
                inputMode="numeric"
                autoComplete="one-time-code"
                placeholder="123456"
                value={code}
                onChange={(e) => setCode(e.target.value)}
              />
            </div>
          </Field>
        )}
        {error && <p className="text-[13px] text-bad">{error}</p>}
        <div className="flex justify-end gap-2">
          <Button type="button" variant="ghost" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button variant="danger" disabled={!ready || revoke.isPending}>
            {revoke.isPending ? (remove ? "Removing…" : "Revoking…") : `${verb} node`}
          </Button>
        </div>
      </form>
    </Modal>
  );
}

/** A DNS label: what the server accepts (D-084). */
const LABEL_RE = /^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$/;

/**
 * Renames a node: its console name and its local DNS name. The machine's
 * hostname is not changed; a new DNS name reaches every node's hosts file.
 */
export function RenameNodeDialog({ node, open, onOpenChange }: { node: Server; open: boolean; onOpenChange(o: boolean): void }) {
  const rename = useRenameServer();
  const setDns = useSetDnsName();
  const full = useServer(node.id).data;
  const dns = full?.dns ?? node.dns;
  const toast = useToast();
  const [name, setName] = useState("");
  const [label, setLabel] = useState("");
  useEffect(() => {
    if (open) {
      setName(nodeName(node));
      setLabel(dns?.label_set ? (dns.label ?? "") : "");
    }
  }, [open, node, dns?.label_set, dns?.label]);
  const labelOk = label === "" || (LABEL_RE.test(label) && label !== "localhost" && label !== "local");
  const suffix = dns?.name && dns.label ? dns.name.slice(dns.label.length) : ".local";
  const busy = rename.isPending || setDns.isPending;
  return (
    <Modal open={open} onOpenChange={onOpenChange} title="Rename node" description="Changes the name shown in Forge and the node's local DNS name. The machine's hostname, SSH and agent identity stay the same.">
      <form
        className="space-y-4"
        onSubmit={async (e) => {
          e.preventDefault();
          if (!labelOk) return;
          try {
            if (name.trim() !== nodeName(node)) await rename.mutateAsync({ id: node.id, name: name.trim() });
            const wasSet = dns?.label_set ? (dns.label ?? "") : "";
            if (label !== wasSet) await setDns.mutateAsync({ id: node.id, label });
            toast("Node renamed");
            onOpenChange(false);
          } catch (err) {
            toast(errorText(err), "error");
          }
        }}
      >
        <Field label="Name" hint={`Leave empty to use the hostname (${node.hostname}).`}>
          <Input value={name} onChange={(e) => setName(e.target.value)} maxLength={63} autoFocus />
        </Field>
        <Field
          label="DNS name"
          hint={
            labelOk
              ? `Every node resolves ${label || dns?.label || "this name"}${suffix}. Leave empty to derive it from the name.`
              : "Use lowercase letters, digits and -, not at the start or end (max 63)."
          }
        >
          <Input value={label} onChange={(e) => setLabel(e.target.value.toLowerCase())} maxLength={63} placeholder={dns?.label ?? "derived from the name"} aria-invalid={!labelOk} />
        </Field>
        <div className="flex justify-end gap-2">
          <Button type="button" variant="ghost" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button variant="primary" disabled={busy || !labelOk}>
            Save
          </Button>
        </div>
      </form>
    </Modal>
  );
}
