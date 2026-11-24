import { Globe, Lock, LockOpen, TriangleAlert, X } from "lucide-react";
import { type ReactNode, useState } from "react";

import { ApiError } from "../api/client";
import {
  type AllowedHostsView,
  type PublicAddressView,
  useClearPublicAddress,
  useResetAllowedHosts,
  useServerSettings,
  useSetAllowedHosts,
  useSetPublicAddress,
} from "../api/server";
import { errorText, useToast } from "../app/toast";
import { SettingsTabs } from "../components/SettingsTabs";
import {
  Badge,
  Button,
  Confirm,
  ErrorNote,
  Field,
  Input,
  Mono,
  PageHeader,
  Panel,
  Skeleton,
  UnsavedBar,
} from "../components/ui";

/** Edits the runtime public address. Keyed on the stored value, so a save or clear resets it. */
function PublicAddressForm({ view }: { view: PublicAddressView }) {
  const [value, setValue] = useState(view.setting ?? "");
  const [confirmClear, setConfirmClear] = useState(false);
  const save = useSetPublicAddress();
  const clear = useClearPublicAddress();
  const toast = useToast();
  const trimmed = value.trim();
  const https = trimmed
    ? trimmed.toLowerCase().startsWith("https://")
    : view.https;
  const fallback =
    view.configured ?? view.last_admin ?? "the address each request came in on";

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
        label="Pinned public address (optional)"
        hint={
          <>
            Usually not needed: the console works on any name or IP of this
            machine, and the pages you open use that address. Pin one, for
            example <Mono>https://forge.example.com</Mono> (scheme, host and
            optional port, no path), when alert emails, webhook payloads and
            OIDC must always use it. Empty uses{" "}
            {view.configured ? (
              <>
                <Mono>public_url</Mono> from forge.toml (
                <Mono>{view.configured}</Mono>)
              </>
            ) : (
              "the address an administrator last signed in on"
            )}
            .
          </>
        }
      >
        <Input
          value={value}
          onChange={(e) => setValue(e.target.value)}
          placeholder={view.configured ?? "https://forge.example.com"}
          spellCheck={false}
          autoComplete="off"
        />
      </Field>
      <div className="flex flex-wrap items-center gap-2 text-xs text-muted">
        {https ? (
          <Lock className="size-3.5 text-ok" />
        ) : (
          <LockOpen className="size-3.5 text-warn" />
        )}
        {https
          ? "Links use HTTPS."
          : "Links use plain HTTP. Publish Forge through an ingress endpoint with a certificate to use HTTPS."}
      </div>
      {view.setting && (
        <Button
          type="button"
          onClick={() => setConfirmClear(true)}
          disabled={clear.isPending}
        >
          Unpin
        </Button>
      )}
      <UnsavedBar
        dirty={trimmed !== (view.setting ?? "")}
        saving={save.isPending}
        onDiscard={() => setValue(view.setting ?? "")}
      />
      {confirmClear && (
        <Confirm
          open
          onOpenChange={setConfirmClear}
          title="Unpin the public address?"
          body={`Links go back to ${fallback}.`}
          confirmLabel="Clear"
          busy={clear.isPending}
          onConfirm={() =>
            clear.mutate(undefined, {
              onSuccess: (v) => {
                setConfirmClear(false);
                toast(
                  v.effective
                    ? `Links now use ${v.effective}`
                    : "Address unpinned",
                );
              },
              onError: (e) => toast(errorText(e), "error"),
            })
          }
        />
      )}
    </form>
  );
}

/** Edits the hosts the console answers on. Keyed on the stored list, so a save resets it. */
function AllowedHostsForm({
  view,
  pinned,
}: {
  view: AllowedHostsView;
  pinned: boolean;
}) {
  const [hosts, setHosts] = useState<string[]>(view.hosts);
  const [draft, setDraft] = useState("");
  const [confirmReset, setConfirmReset] = useState(false);
  const save = useSetAllowedHosts();
  const reset = useResetAllowedHosts();
  const toast = useToast();
  const dirty = hosts.join("\n") !== view.hosts.join("\n");
  const any = hosts.includes("*");

  const add = () => {
    const parts = draft
      .split(/[\s,]+/)
      .map((h) => h.trim().toLowerCase())
      .filter(Boolean);
    if (!parts.length) return;
    setHosts((cur) => [...cur, ...parts.filter((p) => !cur.includes(p))]);
    setDraft("");
  };

  const submit = () => {
    if (!hosts.length)
      return toast("Keep at least one host, or use * for any.", "error");
    save.mutate(hosts, {
      onSuccess: (v) =>
        toast(
          v.any
            ? "The console answers on any host"
            : `The console answers on ${v.hosts.join(", ")}`,
        ),
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
        label="Allowed hosts"
        hint={
          <>
            Host names and IPs the console answers on: <Mono>*</Mono> (any, the
            default), <Mono>forge.example.com</Mono>, <Mono>192.0.2.10</Mono> or{" "}
            <Mono>*.example.com</Mono>. Other hosts get 421. Keep the address
            you are on in the list. Agents are not affected.
          </>
        }
      >
        <div className="flex flex-wrap items-center gap-1.5">
          {hosts.map((h) => (
            <span
              key={h}
              className="inline-flex items-center gap-1 rounded border border-line bg-panel-2 px-1.5 py-0.5 font-mono text-xs"
            >
              {h}
              <button
                type="button"
                aria-label={`Remove ${h}`}
                className="text-muted hover:text-bad"
                onClick={() => setHosts((cur) => cur.filter((x) => x !== h))}
              >
                <X className="size-3" />
              </button>
            </span>
          ))}
          <Input
            className="w-56"
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" || e.key === ",") {
                e.preventDefault();
                add();
              }
            }}
            onBlur={add}
            placeholder="Add a host, Enter"
            spellCheck={false}
            autoComplete="off"
          />
        </div>
      </Field>
      {any && pinned && (
        <div className="flex items-start gap-2 rounded-md border border-warn/40 bg-warn-soft px-3 py-2 text-xs text-warn">
          <TriangleAlert className="mt-0.5 size-3.5 shrink-0" />
          <span>
            An address is pinned but the console still answers on any host.
            Replace <Mono>*</Mono> with the pinned host (and the names you use)
            to narrow it.
          </span>
        </div>
      )}
      {any && !pinned && (
        <div className="flex items-start gap-2 rounded-md border border-warn/40 bg-warn-soft px-3 py-2 text-xs text-warn">
          <TriangleAlert className="mt-0.5 size-3.5 shrink-0" />
          <span>
            With <Mono>*</Mono> there is no DNS-rebinding protection. Narrow the
            list to the names you use.
          </span>
        </div>
      )}
      {!view.any && (
        <Button
          type="button"
          onClick={() => setConfirmReset(true)}
          disabled={reset.isPending}
        >
          Allow any host
        </Button>
      )}
      <UnsavedBar
        dirty={dirty}
        saving={save.isPending}
        onDiscard={() => setHosts(view.hosts)}
      />
      {confirmReset && (
        <Confirm
          open
          onOpenChange={setConfirmReset}
          title="Allow any host?"
          body="The console answers on every name and IP of this machine again (*)."
          confirmLabel="Allow any"
          busy={reset.isPending}
          onConfirm={() =>
            reset.mutate(undefined, {
              onSuccess: () => {
                setConfirmReset(false);
                toast("The console answers on any host");
              },
              onError: (e) => toast(errorText(e), "error"),
            })
          }
        />
      )}
    </form>
  );
}

const SOURCE_LABEL: Record<PublicAddressView["source"], string> = {
  setting: "pinned",
  config: "forge.toml",
  last_admin: "not pinned",
  none: "not pinned",
};

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
      <PageHeader
        eyebrow="Administration"
        title="Settings"
        description="Settings of this Forge server. Your own account and preferences are under Profile."
      />
      <SettingsTabs />
      {denied ? (
        <Panel title="Server settings">
          <p className="p-4 text-[13px] text-muted">
            Only platform administrators can see and change server settings.
          </p>
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
            description="Optional. The console and nodes use whatever address they reach this server on; pin one when links sent out of band must use a fixed address."
            actions={
              <Badge
                tone={s.public_address.source === "setting" ? "ok" : "muted"}
              >
                {SOURCE_LABEL[s.public_address.source]}
              </Badge>
            }
          >
            <PublicAddressForm
              key={s.public_address.setting ?? ""}
              view={s.public_address}
            />
            <div className="border-t border-line py-2">
              <Row label="Links in emails and alerts">
                {s.public_address.effective ? (
                  <span className="inline-flex items-center gap-1.5">
                    <Globe className="size-3.5 text-muted" />
                    <Mono>{s.public_address.effective}</Mono>
                  </span>
                ) : (
                  <span className="text-muted">
                    none yet: no address is pinned and no administrator has
                    signed in
                  </span>
                )}
              </Row>
              <Row label="Last admin address">
                {s.public_address.last_admin ? (
                  <Mono>{s.public_address.last_admin}</Mono>
                ) : (
                  <span className="text-muted">none yet</span>
                )}
              </Row>
              <Row label="Allowed browser origins">
                <Mono>{s.public_address.allowed_origins.join(", ")}</Mono>
              </Row>
            </div>
          </Panel>
          <Panel
            title="Allowed hosts"
            description="Which names and IPs the console answers on."
            actions={
              <Badge tone={s.allowed_hosts.any ? "muted" : "ok"}>
                {s.allowed_hosts.any
                  ? "any"
                  : `${s.allowed_hosts.hosts.length} hosts`}
              </Badge>
            }
          >
            <AllowedHostsForm
              key={s.allowed_hosts.hosts.join("\n")}
              view={s.allowed_hosts}
              pinned={
                s.public_address.source === "setting" ||
                s.public_address.source === "config"
              }
            />
          </Panel>
          <Panel
            title="From forge.toml"
            description="Read-only. Change these in the configuration file and restart forge-server."
          >
            <div className="py-2">
              <Row label="public_url">
                {s.config.public_url ? (
                  <Mono>{s.config.public_url}</Mono>
                ) : (
                  <span className="text-muted">not set</span>
                )}
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
                <Mono>
                  {s.config.trusted_proxies.length
                    ? s.config.trusted_proxies.join(", ")
                    : "none"}
                </Mono>
              </Row>
            </div>
          </Panel>
        </div>
      )}
    </>
  );
}
