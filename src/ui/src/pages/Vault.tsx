import { KeyRound, Trash2 } from "lucide-react";
import { useState } from "react";

import { ApiError } from "../api/client";
import { useDeleteSecret, useSecrets } from "../api/hooks";
import type { SecretMeta } from "../api/types";
import { errorText, useToast } from "../app/toast";
import { Credentials } from "../components/vault/Credentials";
import { Badge, Confirm, Empty, ErrorNote, RowMenu, type RowAction, PageHeader, Panel, Skeleton, Table, Td, ago, time, useRowMenu } from "../components/ui";

export function Vault() {
    const secrets = useSecrets();
  const del = useDeleteSecret();
  const toast = useToast();
  const [deleting, setDeleting] = useState<SecretMeta>();
  const forbidden = secrets.error instanceof ApiError && secrets.error.status === 403;
  const [tab, setTab] = useState<"credentials" | "system">("credentials");
  const rm = useRowMenu();
  const secretActions = (s: SecretMeta): RowAction[] => [{ label: "Delete credential", icon: Trash2, danger: true, onSelect: () => setDeleting(s) }];

  return (
    <>
      <PageHeader
        eyebrow="Administration"
        title="Credential vault"
        description="Accounts for your nodes, services and clusters, for platform administrators only. Passwords are encrypted; revealing one needs an MFA check and is recorded in the audit log."
      />
      <div className="mb-4 flex gap-1 border-b border-line">
        {(
          [
            ["credentials", "Credentials"],
            ["system", "System secrets"],
          ] as const
        ).map(([id, label]) => (
          <button key={id} onClick={() => setTab(id)} className={`-mb-px border-b-2 px-3 py-2 text-[13px] ${tab === id ? "border-accent text-accent" : "border-transparent text-muted hover:text-fg"}`}>
            {label}
          </button>
        ))}
      </div>
      <div key={tab} className="view-enter">
      {tab === "credentials" ? (
        <Credentials />
      ) : (
      <Panel description="Server logins Forge uses for onboarding and repairs. They are never shown. Temporary entries delete themselves after onboarding or within one hour.">
        {forbidden ? (
          <Empty icon={<KeyRound className="size-5" />} title="Admins only" />
        ) : secrets.isLoading ? (
          <Skeleton rows={3} />
        ) : secrets.error ? (
          <ErrorNote error={secrets.error} />
        ) : (secrets.data ?? []).length === 0 ? (
          <Empty icon={<KeyRound className="size-5" />} title="The vault is empty">
            Nothing is stored. Credentials appear here only while an onboarding job runs, or if you chose to keep one for repairs.
          </Empty>
        ) : (
          <Table head={["Login", "Kind", "Retention", "Added", ""]}>
            {secrets.data?.map((s) => (
              <tr key={s.id} {...rm.rowProps(secretActions(s), `Actions for ${s.label}`)}>
                <Td className="font-mono text-xs">{s.label}</Td>
                <Td>{s.kind}</Td>
                <Td>{s.expires_at ? <Badge>auto-deletes {ago(s.expires_at)}</Badge> : <Badge tone="warn">kept for repairs</Badge>}</Td>
                <Td className="text-muted" title={time(s.created_at)}>
                  {ago(s.created_at)}
                </Td>
                <Td className="text-right">
                  <RowMenu actions={secretActions(s)} label={`Actions for ${s.label}`} />
                </Td>
              </tr>
            ))}
          </Table>
        )}
        {rm.menu}
      </Panel>
      )}
      </div>
      <Confirm
        open={!!deleting}
        onOpenChange={(o) => !o && setDeleting(undefined)}
        title="Delete this credential?"
        body="Forge will no longer be able to repair the node over SSH. Nodes keep working through their agents."
        confirmLabel="Delete"
        busy={del.isPending}
        onConfirm={() =>
          deleting &&
          del.mutate(deleting.id, {
            onSuccess: () => {
              toast("Credential deleted");
              setDeleting(undefined);
            },
            onError: (e) => toast(errorText(e), "error"),
          })
        }
      />
    </>
  );
}
