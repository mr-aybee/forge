import { Globe, Lock, LockOpen } from "lucide-react";
import { type ReactNode, useState } from "react";

import { ApiError } from "../api/client";
import { type PublicAddressView, useClearPublicAddress, useServerSettings, useSetPublicAddress } from "../api/server";
import { errorText, useToast } from "../app/toast";
import { SettingsTabs } from "../components/SettingsTabs";
import { Badge, Button, Confirm, ErrorNote, Field, Input, Mono, PageHeader, Panel, Skeleton, UnsavedBar } from "../components/ui";

/** Edits the runtime public address. Keyed on the stored value, so a save or clear resets it. */
function PublicAddressForm({ view }: { view: PublicAddressView }) {
  const [value, setValue] = useState(view.setting ?? "");
  const [confirmClear, setConfirmClear] = useState(false);
  const save = useSetPublicAddress();
  const clear = useClearPublicAddress();
  const toast = useToast();
  const trimmed = value.trim();
  const https = trimmed ? trimmed.toLowerCase().startsWith("https://") : view.https;

  const submit = () => {
    if (!trimmed) return setConfirmClear(true);
    save.mutate(trimmed, {
      onSuccess: (v) => toast(`Links now use ${v.effective}`),
      onError: (e) => toast(errorText(e), "error"),
    });
  };

  return (
    <form
      className="space-y-3 p-4"
      onSubmit={(e) => {
        e.preventDefault();
        submit();
      }}
    >
      <Field
        label="Public address"
        hint={
          <>
            The address people open, for example <Mono>https://forge.example.com</Mono>: scheme, host and optional port, no path. Empty uses{" "}
            <Mono>public_url</Mono> from forge.toml (<Mono>{view.configured}</Mono>). Alert emails, webhook payloads, approval and device sign-in links use it. The
            direct address keeps working.
          </>
        }
      >
        <Input value={value} onChange={(e) => setValue(e.target.value)} placeholder={view.configured} spellCheck={false} autoComplete="off" />
      </Field>
      <div className="flex flex-wrap items-center gap-2 text-xs text-muted">
        {https ? <Lock className="size-3.5 text-ok" /> : <LockOpen className="size-3.5 text-warn" />}
        {https ? "Links use HTTPS." : "Links use plain HTTP. Publish Forge through an ingress endpoint with a certificate to use HTTPS."}
      </div>
      {view.setting && (
        <Button type="button" onClick={() => setConfirmClear(true)} disabled={clear.isPending}>
          Use forge.toml value
        </Button>
      )}
      <UnsavedBar dirty={trimmed !== (view.setting ?? "")} saving={save.isPending} onDiscard={() => setValue(view.setting ?? "")} />
      {confirmClear && (
        <Confirm
          open
          onOpenChange={setConfirmClear}
          title="Clear the public address?"
          body={`Links go back to ${view.configured}.`}
          confirmLabel="Clear"
          busy={clear.isPending}
          onConfirm={() =>
            clear.mutate(undefined, {
              onSuccess: (v) => {
                setConfirmClear(false);
                toast(`Links now use ${v.effective}`);
              },
              onError: (e) => toast(errorText(e), "error"),
            })
          }
        />
      )}
    </form>
  );
}

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="grid grid-cols-[180px_1fr] gap-3 px-4 py-2 text-[13px]">
      <span className="text-muted">{label}</span>
      <span className="min-w-0 break-all">{children}</span>
    </div>
  );
}

/** Server-level settings of this Forge instance (D-120). */
export function ServerSettings() {
  const q = useServerSettings();
  const denied = q.error instanceof ApiError && q.error.status === 403;
  const s = q.data;
  return (
    <>
      <PageHeader eyebrow="Administration" title="Settings" description="Settings of this Forge server. Your own account and preferences are under Profile." />
      <SettingsTabs />
      {denied ? (
        <Panel title="Server settings">
          <p className="p-4 text-[13px] text-muted">Only platform administrators can see and change server settings.</p>
        </Panel>
      ) : q.isError ? (
        <Panel title="Server settings">
          <ErrorNote error={q.error} />
        </Panel>
      ) : !s ? (
        <Panel title="Server settings">
          <Skeleton rows={4} />
        </Panel>
      ) : (
        <div className="space-y-4">
          <Panel
            title="Public address"
            description="Where people reach this console, also when it is published on a domain through an ingress."
            actions={<Badge tone={s.public_address.source === "setting" ? "ok" : "muted"}>{s.public_address.source === "setting" ? "setting" : "forge.toml"}</Badge>}
          >
            <PublicAddressForm key={s.public_address.setting ?? ""} view={s.public_address} />
            <div className="border-t border-line py-2">
              <Row label="In use">
                <span className="inline-flex items-center gap-1.5">
                  <Globe className="size-3.5 text-muted" />
                  <Mono>{s.public_address.effective}</Mono>
                </span>
              </Row>
              <Row label="Allowed browser origins">
                <Mono>{s.public_address.allowed_origins.join(", ")}</Mono>
              </Row>
            </div>
          </Panel>
          <Panel title="From forge.toml" description="Read-only. Change these in the configuration file and restart forge-server.">
            <div className="py-2">
              <Row label="public_url">
                <Mono>{s.config.public_url}</Mono>
              </Row>
              <Row label="node_public_url">
                <Mono>{s.config.node_public_url}</Mono>
              </Row>
              <Row label="agent_public_url">
                <Mono>{s.config.agent_public_url}</Mono>
              </Row>
              <Row label="cookie_secure">
                <Mono>{String(s.config.cookie_secure)}</Mono>
              </Row>
              <Row label="[security] trusted_proxies">
                <Mono>{s.config.trusted_proxies.length ? s.config.trusted_proxies.join(", ") : "none"}</Mono>
              </Row>
            </div>
          </Panel>
        </div>
      )}
    </>
  );
}
