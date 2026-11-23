import { ApiKeysPanel, BusinessSettingsPanel, InstancePanel, ServiceAccountsPanel, SignInPanel, useIsWorkspaceAdmin } from "../components/SettingsSections";
import { SettingsLayout, type SettingsGroup } from "../components/SettingsLayout";
import { MyTunnels } from "../components/tunnels/MyTunnels";
import { ConnectedAppsPanel, RegisteredConnectorsPanel } from "../components/ConnectedApps";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Navigate, useParams } from "@tanstack/react-router";
import { Building2, Cable, Fingerprint, KeyRound, Loader2, LockKeyhole, Moon, Pencil, PlugZap, Plus, Server, ShieldCheck, Sun, Trash2, UserRound } from "lucide-react";
import QRCode from "qrcode";
import { type ReactNode, useEffect, useState } from "react";

import { api, post } from "../api/client";
import { useBeginTotp, useCapabilities, useChangePassword, useConfirmTotp, useMe, useUpdateProfile, useVersion } from "../api/hooks";
import type { User } from "../api/types";
import { usePrefs } from "../app/prefs";
import { errorText, useToast } from "../app/toast";
import { Badge, Button, Confirm, Empty, ErrorNote, Field, Input, Modal, PageHeader, Panel, RowMenu, type RowAction, Skeleton, UnsavedBar, ago, time, useRowMenu } from "../components/ui";
import { PASSKEYS_KEY, createPasskey, isPasskeyCancel, passkeyErrorText, passkeySupport, usePasskeys, type PasskeyInfo } from "../lib/webauthn";

function PasskeysPanel({ className = "" }: { className?: string }) {
  const me = useMe();
  const keys = usePasskeys(me.data?.session_kind !== "bearer");
  const qc = useQueryClient();
  const toast = useToast();
  const support = passkeySupport();
  const [adding, setAdding] = useState(false);
  const [name, setName] = useState("");
  const [renaming, setRenaming] = useState<PasskeyInfo>();
  const [newName, setNewName] = useState("");
  const [removing, setRemoving] = useState<PasskeyInfo>();
  const refresh = () => qc.invalidateQueries({ queryKey: PASSKEYS_KEY });
  const rm = useRowMenu();
  const keyActions = (k: PasskeyInfo): RowAction[] => [
    {
      label: "Rename",
      icon: Pencil,
      onSelect: () => {
        setNewName(k.name);
        setRenaming(k);
      },
    },
    { label: "Remove", icon: Trash2, danger: true, onSelect: () => setRemoving(k) },
  ];

  const add = useMutation({
    mutationFn: async (n: string) => {
      const options = await post<unknown>("/api/v1/me/passkeys/register/begin");
      const body = await createPasskey(options, n);
      return post<PasskeyInfo>("/api/v1/me/passkeys/register/finish", body);
    },
    onSuccess: () => {
      toast("Passkey added");
      setAdding(false);
      void refresh();
    },
    onError: (e) => {
      if (!isPasskeyCancel(e)) toast(passkeyErrorText(e), "error");
    },
  });
  const rename = useMutation({
    mutationFn: ({ id, name }: { id: string; name: string }) => api("PATCH", `/api/v1/me/passkeys/${encodeURIComponent(id)}`, { name }),
    onSuccess: () => {
      toast("Passkey renamed");
      setRenaming(undefined);
      void refresh();
    },
    onError: (e) => toast(errorText(e), "error"),
  });
  const remove = useMutation({
    mutationFn: (id: string) => api("DELETE", `/api/v1/me/passkeys/${encodeURIComponent(id)}`),
    onSuccess: () => {
      toast("Passkey removed");
      setRemoving(undefined);
      void refresh();
    },
    onError: (e) => toast(errorText(e), "error"),
  });

  const data = keys.data;
  const items = data?.items ?? [];
  const unavailable = data ? !data.available : false;
  const bearer = me.data?.session_kind === "bearer";
  const why = unavailable ? "This server does not support passkeys yet." : bearer ? "Passkeys are added in the console, not with an API key." : !support.ok ? support.reason : undefined;
  const openAdd = () => {
    setName("");
    setAdding(true);
  };

  return (
    <Panel
      title="Passkeys"
      description="A passkey signs you in without a password and counts as MFA."
      className={className}
      actions={
        <Button onClick={openAdd} disabled={!!why || add.isPending} title={why}>
          <Plus className="size-4" /> Add passkey
        </Button>
      }
    >
      {why && !unavailable && <p className="border-b border-line px-4 py-2 text-xs text-warn">{why}</p>}
      {keys.isLoading ? (
        <Skeleton rows={2} />
      ) : unavailable ? (
        <p className="px-4 py-3 text-[13px] text-muted">{why}</p>
      ) : data?.error ? (
        <ErrorNote error={data.error} />
      ) : items.length === 0 ? (
        <Empty icon={<Fingerprint className="size-5" />} title="No passkeys yet">
          Add one to sign in with Touch ID, Windows Hello, your phone or a security key.
        </Empty>
      ) : (
        <ul className="divide-y divide-line">
          {items.map((k) => (
            <li key={k.id} className="flex items-center gap-3 px-4 py-2" {...rm.rowProps(keyActions(k), `Actions for ${k.name}`)}>
              <Fingerprint className="size-3.5 shrink-0 text-muted" />
              <span className="min-w-0 flex-1 truncate text-[13px] font-medium" title={k.name}>
                {k.name}
              </span>
              <span className="hidden shrink-0 text-xs text-muted sm:inline" title={time(k.created_at)}>
                added {k.created_at ? ago(k.created_at) : "—"}
              </span>
              <span className="w-28 shrink-0 text-right text-xs text-muted" title={k.last_used_at ? time(k.last_used_at) : undefined}>
                {k.last_used_at ? `used ${ago(k.last_used_at)}` : "never used"}
              </span>
              <RowMenu label={`Actions for ${k.name}`} actions={keyActions(k)} />
            </li>
          ))}
        </ul>
      )}
      {rm.menu}

      <Modal open={adding} onOpenChange={(o) => !add.isPending && setAdding(o)} title="Add a passkey" description="Your browser asks you to confirm with your device or security key.">
        <form
          className="space-y-4"
          onSubmit={(e) => {
            e.preventDefault();
            add.mutate(name.trim());
          }}
        >
          <Field label="Name" hint="So you can tell your passkeys apart.">
            <Input autoFocus required maxLength={60} placeholder="MacBook Touch ID" value={name} onChange={(e) => setName(e.target.value)} />
          </Field>
          <div className="flex justify-end gap-2">
            <Button type="button" variant="ghost" onClick={() => setAdding(false)} disabled={add.isPending}>
              Cancel
            </Button>
            <Button variant="primary" disabled={add.isPending || !name.trim()}>
              {add.isPending && <Loader2 className="size-4 animate-spin" />}
              {add.isPending ? "Waiting for your device…" : "Continue"}
            </Button>
          </div>
        </form>
      </Modal>

      <Modal open={!!renaming} onOpenChange={(o) => !o && setRenaming(undefined)} title="Rename passkey">
        <form
          className="space-y-4"
          onSubmit={(e) => {
            e.preventDefault();
            if (renaming) rename.mutate({ id: renaming.id, name: newName.trim() });
          }}
        >
          <Field label="Name">
            <Input autoFocus required maxLength={60} value={newName} onChange={(e) => setNewName(e.target.value)} />
          </Field>
          <div className="flex justify-end gap-2">
            <Button type="button" variant="ghost" onClick={() => setRenaming(undefined)}>
              Cancel
            </Button>
            <Button variant="primary" disabled={rename.isPending || !newName.trim() || newName.trim() === renaming?.name}>
              {rename.isPending && <Loader2 className="size-4 animate-spin" />}
              Save
            </Button>
          </div>
        </form>
      </Modal>

      <Confirm
        open={!!removing}
        onOpenChange={(o) => !o && setRemoving(undefined)}
        title="Remove passkey?"
        body={
          removing ? (
            <>
              <strong className="text-fg">{removing.name}</strong> can no longer sign you in or verify sensitive actions.
              {items.length === 1 && !me.data?.mfa_enrolled
                ? " It is your only passkey and you have no authenticator app: you will sign in with your password only."
                : " Your password and other sign-in methods keep working."}
            </>
          ) : (
            ""
          )
        }
        confirmLabel="Remove"
        busy={remove.isPending}
        onConfirm={() => removing && remove.mutate(removing.id)}
      />
    </Panel>
  );
}

function MfaSetup() {
  const me = useMe();
  const begin = useBeginTotp();
  const confirm = useConfirmTotp();
  const toast = useToast();
  const [code, setCode] = useState("");
  const [password, setPassword] = useState("");
  const [qr, setQr] = useState<string>();
  const secret = begin.data;

  useEffect(() => {
    if (secret) void QRCode.toDataURL(secret.otpauth_uri, { margin: 1, width: 180 }).then(setQr);
  }, [secret]);

  if (me.data?.mfa_enrolled) {
    return (
      <div className="flex items-center gap-2 p-4 text-ok">
        <ShieldCheck className="size-5" /> Authenticator app enrolled.
        {!me.data.mfa && <span className="text-warn">This session was not MFA-verified: sign in again with a code for sensitive actions.</span>}
      </div>
    );
  }

  return (
    <div className="space-y-4 p-4">
      {!secret ? (
        <>
          <p className="text-muted">Protect your account with a one-time code. Onboarding nodes with credentials and other sensitive actions require it.</p>
          <Button variant="primary" disabled={begin.isPending} onClick={() => begin.mutate(undefined, { onError: (e) => toast(errorText(e), "error") })}>
            {begin.isPending && <Loader2 className="size-4 animate-spin" />} Set up authenticator app
          </Button>
        </>
      ) : (
        <form
          className="flex flex-wrap items-start gap-6"
          onSubmit={(e) => {
            e.preventDefault();
            confirm.mutate(
              { secret: secret.secret, code, password: password || undefined },
              { onSuccess: () => toast("MFA enabled. Sign in again to get an MFA-verified session."), onError: (err) => toast(errorText(err), "error") },
            );
          }}
        >
          {qr && <img src={qr} alt="QR code for your authenticator app" className="rounded-md border border-line bg-white p-1" width={180} height={180} />}
          <div className="max-w-sm space-y-3">
            <p className="text-muted">Scan the code, or enter this key manually:</p>
            <div className="rounded-md bg-panel-2 p-2 font-mono text-xs break-all">{secret.secret}</div>
            <Field label="Code from the app">
              <Input required inputMode="numeric" pattern="[0-9]{6}" value={code} onChange={(e) => setCode(e.target.value)} autoComplete="one-time-code" />
            </Field>
            {!me.data?.user.oidc_subject && (
              <Field label="Current password" hint="Needed when this is your first sign-in factor (no passkey yet).">
                <Input type="password" autoComplete="current-password" value={password} onChange={(e) => setPassword(e.target.value)} />
              </Field>
            )}
            <Button variant="primary" disabled={confirm.isPending}>
              Verify and enable
            </Button>
          </div>
        </form>
      )}
    </div>
  );
}

function AccountForm({ user }: { user: User }) {
  const update = useUpdateProfile();
  const toast = useToast();
  const [f, setF] = useState({ username: user.username ?? "", email: user.email ?? "", display_name: user.display_name });
  const namesChanged = f.username !== (user.username ?? "") || f.email !== (user.email ?? "");
  const changed = namesChanged || f.display_name !== user.display_name;
  const [currentPassword, setCurrentPassword] = useState("");

  return (
    <form
      className="space-y-3 p-4"
      onSubmit={(e) => {
        e.preventDefault();
        update.mutate(namesChanged && currentPassword ? { ...f, current_password: currentPassword } : f, {
          onSuccess: () => {
            setCurrentPassword("");
            toast("Account updated");
          },
          onError: (err) => toast(errorText(err), "error"),
        });
      }}
    >
      <Field label="Username" hint="Letters, digits, '.', '_' or '-'.">
        <Input
          autoCapitalize="none"
          spellCheck={false}
          autoComplete="username"
          pattern="[A-Za-z0-9][A-Za-z0-9._\-]{2,63}"
          required={!f.email}
          value={f.username}
          onChange={(e) => setF({ ...f, username: e.target.value })}
        />
      </Field>
      <Field label="Email" hint={user.email ? undefined : "Optional. Add one to also sign in with it."}>
        <Input type="email" autoComplete="email" required={!f.username} value={f.email} onChange={(e) => setF({ ...f, email: e.target.value })} />
      </Field>
      <Field label="Display name">
        <Input required maxLength={100} value={f.display_name} onChange={(e) => setF({ ...f, display_name: e.target.value })} />
      </Field>
      {namesChanged && !user.oidc_subject && (
        <Field label="Current password" hint="Changing how you sign in needs your password, or a step-up if you use MFA.">
          <Input type="password" autoComplete="current-password" value={currentPassword} onChange={(e) => setCurrentPassword(e.target.value)} />
        </Field>
      )}
      <UnsavedBar
        dirty={changed}
        saving={update.isPending}
        onDiscard={() => {
          setF({ username: user.username ?? "", email: user.email ?? "", display_name: user.display_name });
          setCurrentPassword("");
        }}
      />
    </form>
  );
}

function PasswordForm({ sso }: { sso: boolean }) {
  const change = useChangePassword();
  const toast = useToast();
  const empty = { current_password: "", new_password: "", repeat: "" };
  const [f, setF] = useState(empty);
  const mismatch = f.repeat !== "" && f.repeat !== f.new_password;

  if (sso) {
    return <p className="p-4 text-muted">This account signs in through single sign-on. Change your password with your identity provider.</p>;
  }
  return (
    <form
      className="space-y-3 p-4"
      onSubmit={(e) => {
        e.preventDefault();
        if (mismatch) return;
        change.mutate(
          { current_password: f.current_password, new_password: f.new_password },
          {
            onSuccess: () => {
              toast("Password changed");
              setF(empty);
            },
            onError: (err) => toast(errorText(err), "error"),
          },
        );
      }}
    >
      <Field label="Current password">
        <Input type="password" required autoComplete="current-password" value={f.current_password} onChange={(e) => setF({ ...f, current_password: e.target.value })} />
      </Field>
      <Field label="New password" hint="At least 12 characters.">
        <Input type="password" required minLength={12} autoComplete="new-password" value={f.new_password} onChange={(e) => setF({ ...f, new_password: e.target.value })} />
      </Field>
      <Field label="Repeat the new password" hint={mismatch ? <span className="text-bad">The passwords do not match.</span> : undefined}>
        <Input type="password" required minLength={12} autoComplete="new-password" value={f.repeat} onChange={(e) => setF({ ...f, repeat: e.target.value })} />
      </Field>
      <div className="flex justify-end">
        <Button variant="primary" disabled={mismatch || change.isPending}>
          {change.isPending ? "Changing…" : "Change password"}
        </Button>
      </div>
    </form>
  );
}

const PERSONAL: SettingsGroup = {
  label: "Personal",
  sections: [
    { id: "general", label: "General", icon: UserRound },
    { id: "security", label: "Password & security", icon: ShieldCheck },
    { id: "tokens", label: "Tokens & API keys", icon: KeyRound },
    { id: "access", label: "Tunnels", icon: Cable },
  ],
};
const WORKSPACE: SettingsGroup = {
  label: "Workspace",
  sections: [
    { id: "instance", label: "Instance", icon: Server },
    { id: "sign-in", label: "Sign-in & MFA policy", icon: LockKeyhole },
    { id: "business", label: "Business settings", icon: Building2 },
    { id: "connectors", label: "Connectors", icon: PlugZap },
  ],
};
const DESCRIPTION: Record<string, string> = {
  general: "Your account and how the console looks.",
  security: "Your password, authenticator app and passkeys.",
  tokens: "Keys and tokens for scripts, CI and connected apps.",
  access: "Your open port tunnels, and how to open one with the CLI.",
  instance: "This control plane's configuration and version.",
  "sign-in": "Sign-in providers and the MFA policy for this server.",
  business: "Shared values for service variables.",
  connectors: "AI connectors registered with this workspace.",
};

function AboutPanel() {
  const version = useVersion();
  const caps = useCapabilities();
  const list = caps.data?.capabilities ?? [];
  return (
    <Panel title="About this control plane">
      <div className="space-y-3 p-4">
        <div className="text-[13px]">
          Version <span className="font-mono text-[11px] tabular-nums">{version.data?.version ?? "—"}</span> · API{" "}
          {version.data?.api_versions?.length ? version.data.api_versions.join(", ") : "—"}
        </div>
        <div className="flex flex-wrap gap-1.5">
          {list.length ? (
            list.map((c) => (
              <Badge key={c.id} tone={c.enabled ? "ok" : "muted"}>
                {c.id}
              </Badge>
            ))
          ) : (
            <span className="text-xs text-faint">—</span>
          )}
        </div>
      </div>
    </Panel>
  );
}

export function Settings() {
  const { section } = useParams({ from: "/profile/$section" });
  const me = useMe();
  const { theme, toggleTheme } = usePrefs();
  const admin = useIsWorkspaceAdmin();
  const groups = admin ? [PERSONAL, WORKSPACE] : [PERSONAL];
  const all = [...PERSONAL.sections, ...WORKSPACE.sections];
  const current = all.find((s) => s.id === section);
  if (!current) return <Navigate to="/profile/$section" params={{ section: "general" }} replace />;

  let body: ReactNode;
  switch (current.id) {
    case "general":
      body = (
        <>
          <Panel
            title="Your account"
            description="Sign in with your username or your email."
            actions={me.data?.mfa ? <Badge tone="ok">MFA verified</Badge> : <Badge tone="warn">password only</Badge>}
          >
            {me.data ? <AccountForm key={me.data.user.id} user={me.data.user} /> : me.isError ? <ErrorNote error={me.error} /> : <Skeleton rows={3} />}
          </Panel>
          <Panel title="Appearance">
            <div className="flex items-center justify-between gap-3 px-4 py-3">
              <span className="text-[13px] text-muted">Theme</span>
              <Button onClick={toggleTheme}>
                {theme === "light" ? <Moon className="size-4" /> : <Sun className="size-4" />}
                {theme === "light" ? "Switch to dark" : "Switch to light"}
              </Button>
            </div>
          </Panel>
        </>
      );
      break;
    case "security":
      body = (
        <>
          <Panel title="Password" description="Change the password you sign in with.">
            {me.data ? <PasswordForm sso={!!me.data.user.oidc_subject} /> : me.isError ? <ErrorNote error={me.error} /> : <Skeleton rows={3} />}
          </Panel>
          <Panel title="Multi-factor authentication" description="An authenticator app code. Updates, terminals, packages and other sensitive actions need an MFA-verified session.">
            <MfaSetup />
          </Panel>
          <PasskeysPanel />
        </>
      );
      break;
    case "tokens":
      body = (
        <>
          <ApiKeysPanel />
          <ServiceAccountsPanel />
          <ConnectedAppsPanel />
        </>
      );
      break;
    case "access":
      body = <MyTunnels />;
      break;
    default:
      body = !admin ? (
        <Panel title={current.label}>
          <p className="px-4 py-3 text-[13px] text-muted">Workspace settings are for administrators. Ask one to change them.</p>
        </Panel>
      ) : current.id === "instance" ? (
        <>
          <InstancePanel />
          <AboutPanel />
        </>
      ) : current.id === "sign-in" ? (
        <SignInPanel />
      ) : current.id === "business" ? (
        <BusinessSettingsPanel />
      ) : (
        <RegisteredConnectorsPanel />
      );
  }

  return (
    <>
      <PageHeader eyebrow="Profile" title={current.label} description={DESCRIPTION[current.id]} />
      <SettingsLayout groups={groups} active={current.id}>
        {body}
      </SettingsLayout>
    </>
  );
}
