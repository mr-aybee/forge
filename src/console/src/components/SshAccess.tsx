import { KeyRound, ShieldCheck, SquareTerminal } from "lucide-react";
import { useState } from "react";

import { useProbeNode, useSaveSsh, useSshInfo } from "../api/hooks";
import type { Server } from "../api/types";
import { useTerminals } from "../app/terminals";
import { errorText, useToast } from "../app/toast";
import { Button, CopyButton, CopyValue, Field, Input, Mono, Panel } from "./ui";

/**
 * SSH endpoint of a node for the browser terminal. The host key is pinned only
 * after a user confirms the fingerprint the probe saw (or at onboarding).
 */
export function SshAccess({ node }: { node: Server }) {
  const info = useSshInfo(node.id);
  const probe = useProbeNode();
  const save = useSaveSsh();
  const toast = useToast();
  const terminals = useTerminals();
  const ep = info.data?.endpoint;
  const [editing, setEditing] = useState(false);
  const [host, setHost] = useState("");
  const [port, setPort] = useState(22);
  const seen = probe.data?.host_key;

  const start = () => {
    setHost(ep?.host ?? node.hostname);
    setPort(ep?.port ?? 22);
    probe.reset();
    setEditing(true);
  };

  return (
    <Panel
      title="SSH access"
      description="Browser terminal sessions use short-lived certificates signed by Forge."
      actions={
        ep && !editing ? (
          <Button variant="primary" onClick={() => terminals.open(node)}>
            <SquareTerminal className="size-4" /> Open terminal
          </Button>
        ) : undefined
      }
    >
      <div className="space-y-3 p-4 text-[13px]">
        {ep && !editing && (
          <>
            <div className="flex items-center justify-between gap-3">
              <span className="text-muted">Endpoint</span>
              <Mono>
                {ep.host}:{ep.port}
              </Mono>
            </div>
            <div className="flex items-center justify-between gap-3">
              <span className="flex items-center gap-1.5 text-muted">
                <ShieldCheck className="size-4 text-ok" /> Pinned host key
              </span>
              <Mono>{ep.host_key_fingerprint}</Mono>
            </div>
            <Button variant="ghost" onClick={start}>
              Change endpoint
            </Button>
          </>
        )}
        {!ep && !editing && (
          <div className="flex flex-wrap items-center justify-between gap-3">
            <p className="text-muted">No SSH endpoint yet. Nodes onboarded with SSH get one automatically.</p>
            <Button onClick={start}>Set up SSH access</Button>
          </div>
        )}
        {editing && (
          <form
            className="space-y-3"
            onSubmit={(e) => {
              e.preventDefault();
              if (!seen) {
                probe.mutate({ host, port }, { onError: (err) => toast(errorText(err), "error") });
                return;
              }
              save.mutate(
                { id: node.id, host, port, host_key_fingerprint: seen.fingerprint },
                {
                  onSuccess: () => {
                    toast("SSH endpoint saved");
                    setEditing(false);
                  },
                  onError: (err) => toast(errorText(err), "error"),
                },
              );
            }}
          >
            <div className="grid grid-cols-[1fr_96px] gap-2">
              <Field label="Host or IP as reachable from Forge">
                <Input value={host} onChange={(e) => { setHost(e.target.value); probe.reset(); }} required />
              </Field>
              <Field label="Port">
                <Input type="number" min={1} max={65535} value={port} onChange={(e) => { setPort(Number(e.target.value)); probe.reset(); }} required />
              </Field>
            </div>
            {seen && (
              <div className="rounded-lg border border-warn/40 bg-warn-soft p-3">
                <p className="font-medium">Confirm the host key</p>
                <p className="mt-1 text-muted">Compare with <Mono>ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub</Mono> on the node.</p>
                <p className="mt-2 flex min-w-0 items-center gap-2">
                  <span className="shrink-0 font-mono text-xs text-muted">{seen.algorithm}</span>
                  <CopyValue text={seen.fingerprint} label="Host key fingerprint" />
                </p>
              </div>
            )}
            <div className="flex justify-end gap-2">
              <Button type="button" variant="ghost" onClick={() => setEditing(false)}>
                Cancel
              </Button>
              <Button variant="primary" disabled={probe.isPending || save.isPending}>
                {seen ? "Fingerprint matches — save" : probe.isPending ? "Reading host key…" : "Read host key"}
              </Button>
            </div>
          </form>
        )}
        {info.data && (
          <details className="text-xs text-muted">
            <summary className="flex cursor-pointer items-center gap-1.5">
              <KeyRound className="size-3.5" /> Trust Forge's SSH CA manually
            </summary>
            <p className="mt-2">
              The installer does this for you. On older nodes, add this key to <Mono>/etc/ssh/forge_user_ca.pub</Mono> and set{" "}
              <Mono>TrustedUserCAKeys /etc/ssh/forge_user_ca.pub</Mono> in sshd_config:
            </p>
            <div className="mt-2 flex items-start gap-2">
              <code className="block flex-1 break-all rounded bg-panel-2 p-2 font-mono">{info.data.ca_public_key}</code>
              <CopyButton text={info.data.ca_public_key} />
            </div>
          </details>
        )}
      </div>
    </Panel>
  );
}
