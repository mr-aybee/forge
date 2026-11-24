import {
  Bot,
  KeyRound,
  ListChecks,
  Loader2,
  Plus,
  ShieldAlert,
  Trash2,
} from "lucide-react";
import { useState } from "react";

import {
  accessErrorText,
  apiKeyList as normKeys,
  serviceAccountTokensKey,
  useApiKeys,
  useCreateApiKey,
  useRevokeApiKey,
  type ApiKey,
} from "../api/accessControl";
import { ApiError, api, get, post } from "../api/client";
import { errorText, useToast } from "../app/toast";
import { TokenAccess, type ScopeGrant } from "./access/TokenAccess";
import {
  Badge,
  Button,
  Confirm,
  CopyButton,
  Empty,
  ErrorNote,
  Field,
  Modal,
  RowMenu,
  type RowAction,
  Input,
  Mono,
  Panel,
  Select,
  Skeleton,
  Table,
  Td,
  ago,
  time,
  useRowMenu,
} from "./ui";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

interface SettingsDoc {
  instance: {
    version: string;
    public_url: string | null;
    node_public_url: string;
    agent_public_url: string;
    cookie_secure: boolean;
    data_dir: string;
  };
  auth: {
    local_passwords: boolean;
    oidc: {
      issuer_url: string;
      client_id: string;
      auto_provision: boolean;
    } | null;
    users: number;
    users_with_mfa: number;
    mfa_required_for: string[];
    /** Whether this server requires MFA (`forge-server mfa on|off`); absent on older servers. */
    mfa_required?: boolean;
    /** `[security] allow_elevated_tokens`; absent on older servers. */
    allow_elevated_tokens?: boolean;
  };
  values: { key: string; value: string }[];
}

export const useSettings = () =>
  useQuery({
    queryKey: ["settings"],
    queryFn: () => get<SettingsDoc>("/api/v1/settings"),
    retry: false,
    staleTime: 30_000,
  });
/** Workspace settings are for administrators: the server answers 403 to everyone else. */
export function useIsWorkspaceAdmin(): boolean {
  const s = useSettings();
  return (
    !!s.data ||
    (s.isError && !(s.error instanceof ApiError && s.error.status === 403))
  );
}

/** Loading or error state for a settings-backed panel, keeping the layout. */
function SettingsState({
  title,
  q,
}: {
  title: string;
  q: ReturnType<typeof useSettings>;
}) {
  return (
    <Panel title={title}>
      {q.isLoading ? (
        <Skeleton rows={3} />
      ) : (
        <ErrorNote error={q.error ?? new Error("Settings are unavailable.")} />
      )}
    </Panel>
  );
}

function Row({ k, v }: { k: string; v: React.ReactNode }) {
  return (
    <div className="flex justify-between gap-4 py-1.5">
      <dt className="text-muted">{k}</dt>
      <dd className="min-w-0 truncate text-right">{v}</dd>
    </div>
  );
}

export function InstancePanel() {
  const q = useSettings();
  const s = q.data;
  if (!s?.instance) return <SettingsState title="Instance" q={q} />;
  return (
    <Panel
      title="Instance"
      description="From forge.toml (bootstrap values; change them there and restart)."
    >
      <dl className="divide-y divide-line px-4 py-2 text-[13px]">
        <Row k="Version" v={<Mono>{s.instance.version}</Mono>} />
        <Row
          k="Console URL"
          v={
            s.instance.public_url ? (
              <Mono>{s.instance.public_url}</Mono>
            ) : (
              <span className="text-muted">
                not pinned: the address each request uses
              </span>
            )
          }
        />
        <Row k="URL for nodes" v={<Mono>{s.instance.node_public_url}</Mono>} />
        <Row
          k="Agent endpoint"
          v={<Mono>{s.instance.agent_public_url}</Mono>}
        />
        <Row
          k="Secure cookies"
          v={
            s.instance.cookie_secure ? (
              <Badge tone="ok">on</Badge>
            ) : (
              <Badge tone="warn">off (plain HTTP)</Badge>
            )
          }
        />
      </dl>
    </Panel>
  );
}

export function SignInPanel() {
  const q = useSettings();
  const s = q.data;
  if (!s?.auth) return <SettingsState title="Sign-in & MFA policy" q={q} />;
  const users = s.auth.users ?? 0;
  const withMfa = s.auth.users_with_mfa ?? 0;
  const pct = users ? Math.round((100 * withMfa) / users) : 0;
  const guarded = Array.isArray(s.auth.mfa_required_for)
    ? s.auth.mfa_required_for
    : [];
  const req = s.auth.mfa_required;
  const elevated = s.auth.allow_elevated_tokens;
  return (
    <Panel
      title="Sign-in & MFA policy"
      description="Local accounts with TOTP or passkeys, single sign-on (OIDC) and OAuth + PKCE for apps."
    >
      <dl className="divide-y divide-line px-4 py-2 text-[13px]">
        <Row k="Local passwords" v={<Badge tone="ok">enabled</Badge>} />
        <Row
          k="Single sign-on (OIDC)"
          v={
            s.auth.oidc ? (
              <span>
                <Badge tone="ok">enabled</Badge>{" "}
                <Mono>{s.auth.oidc.issuer_url}</Mono>
              </span>
            ) : (
              <span className="text-faint">
                not configured — add [oidc] to forge.toml
              </span>
            )
          }
        />
        <Row
          k="MFA on this server"
          v={
            req === undefined ? (
              <span className="text-faint">—</span>
            ) : req ? (
              <Badge tone="ok">required for sensitive actions</Badge>
            ) : (
              <span className="text-xs text-muted">
                each user's choice: protects their sign-in, not required for
                actions
              </span>
            )
          }
        />
        <Row k="MFA adoption" v={`${withMfa} of ${users} users (${pct}%)`} />
        <Row
          k="Actions that need MFA"
          v={
            <span
              className="block truncate text-xs text-muted"
              title={guarded.join(", ")}
            >
              {guarded.length ? guarded.join(", ") : "—"}
            </span>
          }
        />
        <Row
          k="API keys and tokens"
          v={
            <span className="text-xs text-muted">
              act as their owner; only <Badge tone="warn">elevated</Badge> ones
              run MFA-guarded actions
              {elevated === false ? " (disabled on this server)" : ""}
            </span>
          }
        />
      </dl>
      <p className="border-t border-line px-4 py-2 text-xs text-muted">
        Users turn on MFA for their own sign-in in{" "}
        <Mono>Profile → Password &amp; security</Mono>. To also require it for
        sensitive actions, run <Mono>forge-server mfa on</Mono> on the control
        plane host, then restart. Elevated tokens are allowed or refused by{" "}
        <Mono>[security] allow_elevated_tokens</Mono> in <Mono>forge.toml</Mono>
        .
      </p>
    </Panel>
  );
}

export function BusinessSettingsPanel() {
  const qc = useQueryClient();
  const toast = useToast();
  const s = useSettings();
  const [key, setKey] = useState("");
  const [value, setValue] = useState("");
  const put = useMutation({
    mutationFn: (b: { key: string; value: string }) =>
      api<void>("PUT", `/api/v1/settings/${encodeURIComponent(b.key)}`, {
        value: b.value,
      }),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ["settings"] }),
    onError: (e) => toast(errorText(e), "error"),
  });
  const del = useMutation({
    mutationFn: (k: string) =>
      api<void>("DELETE", `/api/v1/settings/${encodeURIComponent(k)}`),
    onSuccess: (_, k) => {
      toast(`${k} deleted`);
      void qc.invalidateQueries({ queryKey: ["settings"] });
    },
    onError: (e) => toast(errorText(e), "error"),
  });
  const rm = useRowMenu();
  const settingActions = (k: string): RowAction[] => [
    {
      label: "Delete",
      icon: Trash2,
      danger: true,
      disabled: del.isPending,
      hint: del.isPending ? "Deleting…" : undefined,
      onSelect: () => del.mutate(k),
    },
  ];
  return (
    <Panel
      title="Business settings"
      description={
        <>
          Shared values, e.g. company.domain or support.email. Use them in
          service variables as <Mono>{"${forge.settings.company.domain}"}</Mono>
          .
        </>
      }
    >
      {s.isLoading && <Skeleton rows={2} />}
      {s.isError && <ErrorNote error={s.error} />}
      {(s.data?.values?.length ?? 0) > 0 && (
        <Table head={["Key", "Value", ""]}>
          {(s.data?.values ?? []).map((v) => (
            <tr
              key={v.key}
              {...rm.rowProps(settingActions(v.key), `Actions for ${v.key}`)}
            >
              <Td>
                <Mono>{v.key}</Mono>
              </Td>
              <Td className="max-w-[320px] truncate" title={v.value}>
                {v.value}
              </Td>
              <Td className="text-right">
                <RowMenu
                  actions={settingActions(v.key)}
                  label={`Actions for ${v.key}`}
                />
              </Td>
            </tr>
          ))}
        </Table>
      )}
      {rm.menu}
      <form
        className="flex flex-wrap items-end gap-2 border-t border-line p-4"
        onSubmit={(e) => {
          e.preventDefault();
          put.mutate(
            { key, value },
            {
              onSuccess: () => {
                setKey("");
                setValue("");
                toast("Saved");
              },
            },
          );
        }}
      >
        <Field label="Key">
          <Input
            value={key}
            onChange={(e) => setKey(e.target.value.toLowerCase())}
            pattern="[a-z][a-z0-9_.\-]{0,63}"
            placeholder="company.domain"
            required
          />
        </Field>
        <div className="min-w-[200px] flex-1">
          <Field label="Value">
            <Input
              value={value}
              onChange={(e) => setValue(e.target.value)}
              placeholder="acme.com"
              required
            />
          </Field>
        </div>
        <Button disabled={put.isPending}>
          {put.isPending ? (
            <Loader2 className="size-4 animate-spin" />
          ) : (
            <Plus className="size-4" />
          )}{" "}
          Set
        </Button>
      </form>
    </Panel>
  );
}

/** The "Elevated" opt-in with its caution; shared by API keys and service-account tokens. */
function ElevatedOption({
  checked,
  onChange,
  disabledWhy,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  disabledWhy?: string;
}) {
  return (
    <div className="w-full space-y-1.5">
      <label
        className={`inline-flex items-center gap-2 text-[13px] ${disabledWhy ? "cursor-not-allowed text-faint" : "cursor-pointer"}`}
        title={disabledWhy}
      >
        <input
          type="checkbox"
          className="accent-accent"
          checked={checked && !disabledWhy}
          disabled={!!disabledWhy}
          onChange={(e) => onChange(e.target.checked)}
        />
        Elevated: may run MFA-guarded actions (updates, exec, packages) without
        a prompt
      </label>
      {disabledWhy && <p className="text-xs text-muted">{disabledWhy}</p>}
      {checked && !disabledWhy && (
        <p
          role="note"
          className="flex items-start gap-1.5 rounded-md bg-warn-soft px-2.5 py-1.5 text-xs text-warn"
        >
          <ShieldAlert className="mt-px size-3.5 shrink-0" />
          <span>
            Anyone holding this token can do everything an MFA-verified session
            of its owner can, with no second factor. Use it only for CI that
            needs it, give it an expiry, and keep it in a secret store. Creating
            it asks you to verify with MFA.
          </span>
        </p>
      )}
    </div>
  );
}

/** "Copy your new token now" box. */
function NewSecret({ secret, onDone }: { secret: string; onDone: () => void }) {
  return (
    <div className="m-4 rounded-lg border border-warn/40 bg-warn-soft p-3 text-[13px]">
      <p className="font-medium">
        Copy your new token now. It is not shown again.
      </p>
      <div className="mt-2 flex items-center gap-2">
        <code className="min-w-0 flex-1 rounded bg-panel p-2 font-mono text-xs break-all select-all">
          {secret}
        </code>
        <CopyButton text={secret} />
      </div>
      <Button className="mt-2" variant="ghost" onClick={onDone}>
        Done
      </Button>
    </div>
  );
}

const elevatedWhy = (allowed: boolean | undefined) =>
  allowed === false
    ? "Elevated tokens are disabled on this server ([security] allow_elevated_tokens = false)."
    : undefined;

function KeyRows({
  keys,
  actions,
  rm,
}: {
  keys: ApiKey[];
  actions: (k: ApiKey) => RowAction[];
  rm: ReturnType<typeof useRowMenu>;
}) {
  return (
    <Table head={["Name", "Key", "Last used", "Expires", ""]}>
      {keys.map((k) => (
        <tr key={k.id} {...rm.rowProps(actions(k), `Actions for ${k.name}`)}>
          <Td className="max-w-[16rem] font-medium">
            <span className="flex min-w-0 items-center gap-1.5">
              <span className="truncate" title={k.name}>
                {k.name}
              </span>
              {k.elevated && (
                <span
                  title="Runs MFA-guarded actions without a prompt"
                  className="shrink-0"
                >
                  <Badge tone="warn">elevated</Badge>
                </span>
              )}
              {k.restrictions && (
                <span
                  title={restrictionTitle(k.restrictions)}
                  className="shrink-0"
                >
                  <Badge tone="accent">Restricted</Badge>
                </span>
              )}
            </span>
            {k.restrictions && (
              <span className="block font-mono text-[11px] font-normal text-faint tabular-nums">
                {restrictionSummary(k.restrictions)}
              </span>
            )}
          </Td>
          <Td>
            <span className="font-mono text-[11px]">
              {k.prefix ? `${k.prefix}_…` : "—"}
            </span>
          </Td>
          <Td
            className="text-xs text-muted"
            title={k.last_used_at ? time(k.last_used_at) : undefined}
          >
            {k.last_used_at ? ago(k.last_used_at) : "never"}
          </Td>
          <Td
            className={`text-xs ${k.expires_at ? "text-muted" : "text-warn"}`}
            title={
              k.expires_at
                ? time(k.expires_at)
                : "This key never expires. Prefer keys that expire."
            }
          >
            {k.expires_at ? time(k.expires_at) : "never"}
          </Td>
          <Td className="text-right">
            <RowMenu actions={actions(k)} label={`Actions for ${k.name}`} />
          </Td>
        </tr>
      ))}
    </Table>
  );
}

function ExpirySelect({
  value,
  onChange,
}: {
  value: string;
  onChange: (v: string) => void;
}) {
  return (
    <Field label="Expires">
      <Select value={value} onChange={(e) => onChange(e.target.value)}>
        <option value="30">in 30 days</option>
        <option value="90">in 90 days</option>
        <option value="365">in a year</option>
        <option value="">never</option>
      </Select>
    </Field>
  );
}

const restrictionSummary = (r: NonNullable<ApiKey["restrictions"]>) => {
  const actions = new Set(r.flatMap((x) => x.actions)).size;
  return `${r.length} scope${r.length === 1 ? "" : "s"} · ${actions} action${actions === 1 ? "" : "s"}`;
};
const restrictionTitle = (r: NonNullable<ApiKey["restrictions"]>) =>
  `Only these actions at these scopes (never more than you hold):\n${r.map((x) => `${x.scope.kind}:${x.scope.id} → ${x.actions.join(", ")}`).join("\n")}`;

export function ApiKeysPanel() {
  const toast = useToast();
  const keys = useApiKeys();
  const revoke = useRevokeApiKey();
  const [creating, setCreating] = useState(false);
  const [shown, setShown] = useState<string>();
  const [revoking, setRevoking] = useState<ApiKey>();
  const rm = useRowMenu();
  const keyActions = (k: ApiKey): RowAction[] => [
    {
      label: "Revoke key",
      icon: Trash2,
      danger: true,
      onSelect: () => setRevoking(k),
    },
  ];
  const list = keys.data ?? [];
  return (
    <Panel
      title="Personal API keys"
      description={
        <>
          For scripts and CI: send <Mono>Authorization: Bearer fk_…</Mono>. A
          key acts as you, with all your access or only the scopes and actions
          you select. Only an elevated key runs MFA-guarded actions; a normal
          key is refused with “step-up required”.
        </>
      }
      actions={
        <Button onClick={() => setCreating(true)}>
          <KeyRound className="size-4" /> New key
        </Button>
      }
    >
      {shown && <NewSecret secret={shown} onDone={() => setShown(undefined)} />}
      {keys.isLoading ? (
        <Skeleton rows={2} />
      ) : keys.isError ? (
        <ErrorNote error={keys.error} />
      ) : list.length > 0 ? (
        <KeyRows keys={list} actions={keyActions} rm={rm} />
      ) : (
        <Empty
          icon={<KeyRound className="size-5" />}
          title="No API keys"
          action={<Button onClick={() => setCreating(true)}>New key</Button>}
        >
          Create a key for a script; limit it to the scopes and actions the
          script needs.
        </Empty>
      )}
      {creating && (
        <NewKeyDialog
          onClose={() => setCreating(false)}
          onCreated={(secret) => {
            setCreating(false);
            setShown(secret ?? undefined);
          }}
        />
      )}
      <Confirm
        open={!!revoking}
        onOpenChange={(o) => !o && !revoke.isPending && setRevoking(undefined)}
        title={`Revoke ${revoking?.name ?? "key"}?`}
        body="Anything using this key stops working immediately."
        confirmLabel="Revoke"
        busy={revoke.isPending}
        onConfirm={() =>
          revoking &&
          revoke.mutate(revoking.id, {
            onSuccess: () => {
              toast("API key revoked");
              setRevoking(undefined);
            },
            onError: (e) => toast(errorText(e), "error"),
          })
        }
      />
      {rm.menu}
    </Panel>
  );
}

/** New personal key: name, expiry, elevation and access (all mine, or only selected scopes and actions). */
/** A new personal access token, or one for `account` (the same kind of key). */
function NewKeyDialog({
  account,
  onClose,
  onCreated,
}: {
  account?: { id: string; name: string };
  onClose: () => void;
  onCreated: (secret: string | null) => void;
}) {
  const toast = useToast();
  const create = useCreateApiKey(account?.id);
  const whose = account ? `${account.name}'s` : "my";
  const holder = account ? "the account holds" : "you hold";
  const allowed = useSettings().data?.auth?.allow_elevated_tokens;
  const [f, setF] = useState<{
    name: string;
    days: string;
    elevated: boolean;
    restricted: boolean;
    grants: ScopeGrant[];
  }>({ name: "", days: "90", elevated: false, restricted: false, grants: [] });
  const set = (p: Partial<typeof f>) => setF((x) => ({ ...x, ...p }));
  const incomplete =
    f.restricted &&
    (f.grants.length === 0 || f.grants.some((g) => g.actions.size === 0));
  const submit = () =>
    create.mutate(
      {
        name: f.name.trim(),
        expires_days: f.days ? Number(f.days) : null,
        elevated: f.elevated && allowed !== false,
        ...(f.restricted
          ? {
              restrictions: f.grants.map((g) => ({
                scope: g.scope,
                actions: [...g.actions].sort(),
              })),
            }
          : {}),
      },
      {
        onSuccess: (r) => {
          toast(account ? "Token issued" : "API key created");
          onCreated(r.secret);
        },
        onError: (e) => toast(accessErrorText(e), "error"),
      },
    );
  return (
    <Modal
      open
      onOpenChange={(o) => !o && !create.isPending && onClose()}
      title={account ? `New token for ${account.name}` : "New API key"}
      description="The secret is shown once, right after you create the key."
      wide
    >
      <form
        className="space-y-3"
        onSubmit={(e) => {
          e.preventDefault();
          submit();
        }}
      >
        <div className="grid gap-3 sm:grid-cols-[1fr_auto]">
          <Field label="Name">
            <Input
              value={f.name}
              onChange={(e) => set({ name: e.target.value })}
              placeholder="GitHub Actions deploy"
              required
              maxLength={80}
              autoFocus
            />
          </Field>
          <ExpirySelect value={f.days} onChange={(days) => set({ days })} />
        </div>
        <fieldset className="space-y-1.5">
          <legend className="mb-1 text-xs font-medium text-muted">
            Access
          </legend>
          <div
            className="inline-flex rounded-md border border-line p-0.5"
            role="radiogroup"
            aria-label="Access"
          >
            {(
              [
                [false, `All ${whose} access`],
                [true, "Only selected"],
              ] as const
            ).map(([v, label]) => (
              <button
                key={label}
                type="button"
                role="radio"
                aria-checked={f.restricted === v}
                onClick={() => set({ restricted: v })}
                className={`h-6 rounded px-2.5 text-xs ${f.restricted === v ? "bg-accent-soft font-medium text-accent" : "text-muted hover:text-fg"}`}
              >
                {label}
              </button>
            ))}
          </div>
          <p className="text-xs text-muted">
            {f.restricted
              ? `The key may only run the actions you pick, at the scopes you pick. It never gets more than ${holder}.`
              : account
                ? "The key can do everything the account can, now and after its access changes."
                : "The key can do everything you can, now and after your roles change."}
          </p>
          {f.restricted && (
            <TokenAccess
              value={f.grants}
              onChange={(grants) => set({ grants })}
            />
          )}
        </fieldset>
        <ElevatedOption
          checked={f.elevated}
          onChange={(elevated) => set({ elevated })}
          disabledWhy={elevatedWhy(allowed)}
        />
        <div className="flex items-center justify-end gap-2">
          {incomplete && (
            <span className="mr-auto text-xs text-warn">
              Add at least one scope, with at least one action each.
            </span>
          )}
          <Button type="button" onClick={onClose}>
            Cancel
          </Button>
          <Button
            variant="primary"
            disabled={create.isPending || !f.name.trim() || incomplete}
          >
            {create.isPending ? (
              <Loader2 className="size-4 animate-spin" />
            ) : (
              <KeyRound className="size-4" />
            )}{" "}
            Create key
          </Button>
        </div>
      </form>
    </Modal>
  );
}

// ------------------------------------------------------------ service accounts

interface ServiceAccount {
  id: string;
  name: string;
  description: string;
  disabled: boolean;
  owner: string | null;
  owner_inactive: boolean;
  tokens: number;
  last_used_at: number | null;
}

function normAccounts(raw: unknown): ServiceAccount[] {
  if (!Array.isArray(raw)) return [];
  return raw.flatMap((x) => {
    const r = x && typeof x === "object" ? (x as Record<string, unknown>) : {};
    if (typeof r.id !== "string" || !r.id) return [];
    const n = (v: unknown) =>
      typeof v === "number" && Number.isFinite(v) ? v : null;
    return [
      {
        id: r.id,
        name: typeof r.name === "string" && r.name ? r.name : r.id.slice(0, 8),
        description: typeof r.description === "string" ? r.description : "",
        disabled: r.disabled === true,
        owner: typeof r.owner === "string" && r.owner ? r.owner : null,
        owner_inactive: r.owner_inactive === true,
        tokens: n(r.tokens) ?? 0,
        last_used_at: n(r.last_used_at),
      },
    ];
  });
}

const SA_KEY = ["service-accounts"] as const;

/** Service accounts; `null` data when the caller may not see them or the server has none (older server). */
function useServiceAccounts() {
  return useQuery({
    queryKey: SA_KEY,
    retry: false,
    staleTime: 15_000,
    queryFn: () =>
      get<unknown>("/api/v1/service-accounts")
        .then(normAccounts)
        .catch((e: unknown) => {
          if (e instanceof ApiError && [403, 404, 405].includes(e.status))
            return null;
          throw e;
        }),
  });
}

export function ServiceAccountsPanel() {
  const q = useServiceAccounts();
  const qc = useQueryClient();
  const toast = useToast();
  const [creating, setCreating] = useState(false);
  const [f, setF] = useState({ name: "", description: "" });
  const [open, setOpen] = useState<ServiceAccount>();
  const rm = useRowMenu();
  const create = useMutation({
    mutationFn: () =>
      post<unknown>("/api/v1/service-accounts", {
        name: f.name.trim(),
        description: f.description.trim(),
      }),
    onSuccess: () => {
      toast(`svc-${f.name.trim()} created`);
      setCreating(false);
      setF({ name: "", description: "" });
      void qc.invalidateQueries({ queryKey: SA_KEY });
    },
    onError: (e) => toast(errorText(e), "error"),
  });
  if (q.data === null) return null;
  const list = q.data ?? [];
  const actions = (a: ServiceAccount): RowAction[] => [
    { label: "Tokens", icon: ListChecks, onSelect: () => setOpen(a) },
  ];

  return (
    <Panel
      title="Service accounts"
      description="Non-human accounts for CI and integrations. Each has an accountable owner and its own tokens; it never signs in interactively."
      actions={
        <Button onClick={() => setCreating(true)}>
          <Plus className="size-4" /> New service account
        </Button>
      }
    >
      {q.isLoading ? (
        <Skeleton rows={2} />
      ) : q.isError ? (
        <ErrorNote error={q.error} />
      ) : list.length === 0 ? (
        <Empty
          icon={<Bot className="size-5" />}
          title="No service accounts"
          action={
            <Button onClick={() => setCreating(true)}>
              New service account
            </Button>
          }
        >
          Create one for CI instead of using a personal key, so access survives
          when people leave.
        </Empty>
      ) : (
        <Table head={["Name", "Owner", "Tokens", "Last used", ""]}>
          {list.map((a) => (
            <tr
              key={a.id}
              {...rm.rowProps(actions(a), `Actions for ${a.name}`)}
            >
              <Td className="max-w-[16rem]">
                <button
                  type="button"
                  className="flex max-w-full min-w-0 items-center gap-1.5 text-left font-medium hover:text-accent"
                  onClick={() => setOpen(a)}
                >
                  <span
                    className="truncate font-mono text-[12px]"
                    title={a.description || a.name}
                  >
                    {a.name}
                  </span>
                  {a.disabled && <Badge tone="muted">disabled</Badge>}
                </button>
              </Td>
              <Td className="max-w-[10rem] text-xs">
                <span
                  className={`block truncate ${a.owner_inactive ? "text-warn" : "text-muted"}`}
                  title={
                    a.owner_inactive
                      ? "The owner is gone or disabled: assign a new one"
                      : (a.owner ?? undefined)
                  }
                >
                  {a.owner ?? "—"}
                  {a.owner_inactive && " (inactive)"}
                </span>
              </Td>
              <Td className="font-mono text-[11px] tabular-nums">{a.tokens}</Td>
              <Td
                className="text-xs text-muted"
                title={a.last_used_at ? time(a.last_used_at) : undefined}
              >
                {a.last_used_at ? ago(a.last_used_at) : "never"}
              </Td>
              <Td className="text-right">
                <RowMenu actions={actions(a)} label={`Actions for ${a.name}`} />
              </Td>
            </tr>
          ))}
        </Table>
      )}
      {rm.menu}

      <Modal
        open={creating}
        onOpenChange={(o) => !create.isPending && setCreating(o)}
        title="New service account"
        description="You become its accountable owner."
      >
        <form
          className="space-y-3"
          onSubmit={(e) => {
            e.preventDefault();
            create.mutate();
          }}
        >
          <Field
            label="Name"
            hint={
              <>
                3–40 of a-z, 0-9 and -. The account is called{" "}
                <Mono>svc-{f.name.trim() || "<name>"}</Mono>.
              </>
            }
          >
            <Input
              autoFocus
              required
              pattern="[a-z0-9][a-z0-9\-]{1,38}[a-z0-9]"
              value={f.name}
              onChange={(e) =>
                setF({ ...f, name: e.target.value.toLowerCase() })
              }
              placeholder="github-deploy"
            />
          </Field>
          <Field label="Description">
            <Input
              maxLength={500}
              value={f.description}
              onChange={(e) => setF({ ...f, description: e.target.value })}
              placeholder="Deploys from the main branch"
            />
          </Field>
          <div className="flex justify-end gap-2">
            <Button
              type="button"
              variant="ghost"
              onClick={() => setCreating(false)}
              disabled={create.isPending}
            >
              Cancel
            </Button>
            <Button
              variant="primary"
              disabled={create.isPending || !f.name.trim()}
            >
              {create.isPending && <Loader2 className="size-4 animate-spin" />}{" "}
              Create
            </Button>
          </div>
        </form>
      </Modal>

      <Modal
        open={!!open}
        onOpenChange={(o) => !o && setOpen(undefined)}
        title={open ? `Tokens of ${open.name}` : "Tokens"}
        description={open?.description || undefined}
        wide
      >
        {open && <ServiceAccountTokens account={open} />}
      </Modal>
    </Panel>
  );
}

function ServiceAccountTokens({ account }: { account: ServiceAccount }) {
  const qc = useQueryClient();
  const toast = useToast();
  const base = `/api/v1/service-accounts/${encodeURIComponent(account.id)}/tokens`;
  const key = serviceAccountTokensKey(account.id);
  const tokens = useQuery({
    queryKey: key,
    queryFn: () => get<unknown>(base).then(normKeys),
  });
  const [issuing, setIssuing] = useState(false);
  const [shown, setShown] = useState<string>();
  const [revoking, setRevoking] = useState<ApiKey>();
  const refresh = () => {
    void qc.invalidateQueries({ queryKey: key });
    void qc.invalidateQueries({ queryKey: SA_KEY });
  };
  const revoke = useMutation({
    mutationFn: (id: string) =>
      api<void>("DELETE", `${base}/${encodeURIComponent(id)}`),
    onSuccess: () => {
      toast("Token revoked");
      setRevoking(undefined);
      refresh();
    },
    onError: (e) => toast(errorText(e), "error"),
  });
  const rm = useRowMenu();
  const actions = (k: ApiKey): RowAction[] => [
    {
      label: "Revoke token",
      icon: Trash2,
      danger: true,
      onSelect: () => setRevoking(k),
    },
  ];
  const list = tokens.data ?? [];
  const disabledWhy = account.disabled
    ? "The service account is disabled."
    : undefined;

  return (
    <div className="-mx-5 -mb-5 border-t border-line">
      {shown && <NewSecret secret={shown} onDone={() => setShown(undefined)} />}
      {tokens.isLoading ? (
        <Skeleton rows={2} />
      ) : tokens.isError ? (
        <ErrorNote error={tokens.error} />
      ) : list.length ? (
        <KeyRows keys={list} actions={actions} rm={rm} />
      ) : (
        <p className="px-5 py-3 text-xs text-muted">
          No tokens. Issue one below.
        </p>
      )}
      {rm.menu}
      <div className="flex justify-end border-t border-line px-5 py-4">
        <Button
          variant="primary"
          onClick={() => setIssuing(true)}
          disabled={!!disabledWhy}
          title={disabledWhy}
        >
          <KeyRound className="size-4" /> Issue token
        </Button>
      </div>
      {issuing && (
        <NewKeyDialog
          account={account}
          onClose={() => setIssuing(false)}
          onCreated={(secret) => {
            setIssuing(false);
            setShown(secret ?? undefined);
            refresh();
          }}
        />
      )}
      <Confirm
        open={!!revoking}
        onOpenChange={(o) => !o && !revoke.isPending && setRevoking(undefined)}
        title={`Revoke ${revoking?.name ?? "token"}?`}
        body={`Anything using this token as ${account.name} stops working immediately.`}
        confirmLabel="Revoke"
        busy={revoke.isPending}
        onConfirm={() => revoking && revoke.mutate(revoking.id)}
      />
    </div>
  );
}
