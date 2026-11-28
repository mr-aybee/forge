import { Link } from "@tanstack/react-router";
import { KeyRound, Loader2, Logs, Plus, Trash2 } from "lucide-react";
import { useState } from "react";

import { analyticsUnavailable } from "../../api/analytics";
import { type CreatedLogKey, type LogKey, useCreateLogIngestKey, useLogIngestKeys, useRevokeLogIngestKey } from "../../api/logs";
import { errorText, useToast } from "../../app/toast";
import { Button, Confirm, CopyButton, CopyIcon, Empty, ErrorNote, Field, Input, Modal, Panel, Skeleton, ago, time } from "../ui";

/** Button that opens the log console filtered to one service. */
export function OpenLogStream({ appId }: { appId: string }) {
  return (
    <Link to="/logs" search={{ services: appId }} className="inline-flex items-center gap-1.5 rounded-md border border-transparent px-3 py-1.5 text-[13px] font-medium text-muted hover:bg-panel-2 hover:text-fg">
      <Logs className="size-4" /> Open log stream
    </Link>
  );
}

/** Keys a service uses to send Seq/CLEF events to Forge. */
export function LogIngestKeys({ appId, appName }: { appId: string; appName: string }) {
  const keys = useLogIngestKeys(appId);
  const revoke = useRevokeLogIngestKey(appId);
  const toast = useToast();
  const [creating, setCreating] = useState(false);
  const [revoking, setRevoking] = useState<LogKey | null>(null);
  const off = analyticsUnavailable(keys.error);
  const list = keys.data ?? [];

  return (
    <Panel
      title={list.length ? `Log ingest keys · ${list.length}` : "Log ingest keys"}
      actions={
        <Button className="h-7 px-2 text-xs" onClick={() => setCreating(true)} disabled={!!off} title={off ? "Analytics is off" : "The service sends structured logs (Seq / CLEF) with one of these keys"}>
          <Plus className="size-3.5" /> New key
        </Button>
      }
    >
      {keys.isPending ? (
        <Skeleton rows={2} />
      ) : off ? (
        <p className="px-4 py-3 text-xs text-muted">{off} Log ingest needs the analytics database.</p>
      ) : keys.isError ? (
        <ErrorNote error={keys.error} />
      ) : list.length === 0 ? (
        <Empty icon={<KeyRound className="size-5" />} title="No ingest keys" action={<Button onClick={() => setCreating(true)}>Create a key</Button>}>
          The service sends structured logs (Seq / CLEF) with a key. Create one and configure the service's logger to send events to Forge.
        </Empty>
      ) : (
        <ul className="divide-y divide-line">
          {list.map((k) => (
            <li key={k.id} className="flex min-w-0 items-center gap-2 px-3 py-1 text-[13px]">
              <KeyRound className="size-3.5 shrink-0 text-faint" />
              <span className="min-w-16 flex-1 truncate font-medium" title={k.name}>
                {k.name}
              </span>
              <span className="shrink-0 font-mono text-[11px] text-muted">{k.prefix}…</span>
              <span className="shrink-0 text-xs whitespace-nowrap text-faint" title={k.last_used_at ? time(k.last_used_at) : "Never used"}>
                {k.last_used_at ? `used ${ago(k.last_used_at)}` : "never used"}
              </span>
              <span className="hidden shrink-0 text-xs whitespace-nowrap text-faint 2xl:inline" title={time(k.created_at)}>
                created {ago(k.created_at)}
              </span>
              <button type="button" aria-label={`Revoke ${k.name}`} title="Revoke" className="grid size-7 place-items-center rounded text-muted hover:bg-bad-soft hover:text-bad" onClick={() => setRevoking(k)}>
                <Trash2 className="size-3.5" />
              </button>
            </li>
          ))}
        </ul>
      )}
      {creating && <CreateKeyDialog appId={appId} appName={appName} onClose={() => setCreating(false)} />}
      {revoking && (
        <Confirm
          open
          onOpenChange={(o) => !o && setRevoking(null)}
          title={`Revoke "${revoking.name}"?`}
          body={
            <>
              Senders using the key <span className="font-mono text-xs">{revoking.prefix}…</span> are rejected from now on and their events are lost. Events already stored stay. This
              cannot be undone; give the service a new key first.
            </>
          }
          confirmLabel="Revoke key"
          busy={revoke.isPending}
          onConfirm={() =>
            revoke.mutate(revoking.id, {
              onSuccess: () => {
                toast(`Revoked ${revoking.name}`);
                setRevoking(null);
              },
              onError: (e) => toast(errorText(e), "error"),
            })
          }
        />
      )}
    </Panel>
  );
}

function CreateKeyDialog({ appId, appName, onClose }: { appId: string; appName: string; onClose: () => void }) {
  const create = useCreateLogIngestKey(appId);
  const toast = useToast();
  const [form, setForm] = useState({ name: `${appName}-logs` });
  // The response is kept by the mutation only while the dialog is mounted; closing drops the token.
  const created: CreatedLogKey | undefined = create.data;

  return (
    <Modal open onOpenChange={(o) => !o && onClose()} title={created ? "Ingest key created" : "New log ingest key"} description={created ? undefined : `For ${appName}.`} wide={!!created}>
      {created ? (
        <Created created={created} onClose={onClose} />
      ) : (
        <form
          className="space-y-4"
          onSubmit={(e) => {
            e.preventDefault();
            create.mutate(form.name.trim(), { onSuccess: () => toast("Key created"), onError: (er) => toast(errorText(er), "error") });
          }}
        >
          <Field label="Name" hint="Where the key is used, e.g. production.">
            <Input autoFocus value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} maxLength={100} required />
          </Field>
          <div className="flex justify-end gap-2">
            <Button type="button" onClick={onClose}>
              Cancel
            </Button>
            <Button type="submit" variant="primary" disabled={create.isPending || !form.name.trim()}>
              {create.isPending && <Loader2 className="size-4 animate-spin" />}
              Create key
            </Button>
          </div>
        </form>
      )}
    </Modal>
  );
}

function Snippet({ label, text }: { label: string; text: string }) {
  return (
    <div>
      <div className="mb-1 flex items-center justify-between text-xs text-muted">
        {label}
        <CopyIcon text={text} label={`Copy ${label}`} />
      </div>
      <pre className="overflow-x-auto rounded-md border border-line bg-term p-2 font-mono text-[11px] leading-relaxed text-term-fg select-text">{text}</pre>
    </div>
  );
}

function Created({ created, onClose }: { created: CreatedLogKey; onClose: () => void }) {
  const origin = window.location.origin;
  const token = created.token ?? "";
  return (
    <div className="space-y-3">
      <p className="rounded-md bg-warn-soft px-3 py-2 text-xs text-warn">Copy the key now. Forge stores only a hash and cannot show it again.</p>
      <div className="flex min-w-0 items-center gap-2">
        <code className="min-w-0 flex-1 truncate rounded-md border border-line bg-panel-2 px-2 py-1.5 font-mono text-xs select-all" title={token}>
          {token}
        </code>
        <CopyButton text={token} />
      </div>
      <Snippet label="Serilog (.NET)" text={`Log.Logger = new LoggerConfiguration()\n    .WriteTo.Seq("${origin}", apiKey: "${token}")\n    .CreateLogger();`} />
      <Snippet
        label="CLEF over HTTP"
        text={`curl -X POST "${origin}/api/events/raw?clef" \\\n  -H "X-Seq-ApiKey: ${token}" \\\n  -H "Content-Type: application/vnd.serilog.clef" \\\n  --data-binary '{"@t":"2026-01-01T12:00:00Z","@l":"Information","@mt":"Hello {User}","User":"alice"}'`}
      />
      <div className="flex justify-end">
        <Button variant="primary" onClick={onClose}>
          Done
        </Button>
      </div>
    </div>
  );
}
