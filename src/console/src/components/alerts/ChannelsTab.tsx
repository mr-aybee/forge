import { Link } from "@tanstack/react-router";
import { Loader2, Pencil, Plus, Send, Trash2, Webhook } from "lucide-react";
import { Fragment, useState } from "react";

import { ApiError } from "../../api/client";
import {
  type Channel,
  type ChannelBody,
  type ChannelKind,
  type DeliveryView,
  NOTIFY_LIMITS,
  type SmtpTls,
  useChannels,
  useDeleteChannel,
  usePolicies,
  useSaveChannel,
  useTestChannel,
} from "../../api/notifications";
import { useVaultItems } from "../../api/vault";
import { errorText, useToast } from "../../app/toast";
import {
  Button,
  Confirm,
  Empty,
  ErrorNote,
  Field,
  IconButton,
  Input,
  Modal,
  Mono,
  Panel,
  Select,
  Skeleton,
  Switch,
  Table,
  Td,
  TextArea,
  ago,
  duration,
  short,
  time,
} from "../ui";
import {
  CHANNEL_ROLE_HINT,
  DeliveryBadge,
  FormError,
  actionError,
  attemptText,
  useNotificationRights,
} from "./parts";

const KIND_LABEL: Record<ChannelKind, string> = {
  webhook: "Webhook",
  slack: "Slack-compatible",
  email: "Email",
};
const TLS_LABEL: Record<SmtpTls, string> = {
  starttls: "STARTTLS (required)",
  tls: "Implicit TLS (SMTPS)",
  none: "None (unencrypted)",
};
const TLS_PORT: Record<SmtpTls, number> = { starttls: 587, tls: 465, none: 25 };

/** Recipients typed one per line or separated by commas or semicolons. */
const splitAddresses = (text: string) => [
  ...new Set(
    text
      .split(/[\n,;]+/)
      .map((a) => a.trim())
      .filter(Boolean),
  ),
];
/** A loose check before the server parses the address for real. */
const looksLikeAddress = (a: string) =>
  /^[^\s@<>]+@[^\s@<>]+$/.test(a.replace(/^.*<([^>]*)>\s*$/, "$1"));

/** Channels: where alarm messages go. The URL is write-only; only its host is shown. */
export function ChannelsTab() {
  const q = useChannels();
  const policies = usePolicies();
  const can = useNotificationRights().channels;
  const del = useDeleteChannel();
  const test = useTestChannel();
  const toast = useToast();
  const [edit, setEdit] = useState<Channel | "new" | null>(null);
  const [deleting, setDeleting] = useState<Channel | null>(null);
  const list = q.data ?? [];
  const usedBy = deleting
    ? (policies.data ?? []).filter((p) => p.channels.includes(deleting.id))
    : [];
  const why = (label: string) =>
    can ? label : `${label}: ${CHANNEL_ROLE_HINT}`;

  const create = (
    <Button
      variant="primary"
      disabled={!can}
      title={can ? undefined : CHANNEL_ROLE_HINT}
      onClick={() => setEdit("new")}
    >
      <Plus className="size-4" /> New channel
    </Button>
  );

  return (
    <Panel
      title="Channels"
      description="A channel is a place alarm messages are sent to: a webhook, a Slack-compatible receiver or email. Policies decide which alarms go to which channel."
      actions={create}
    >
      {q.isLoading ? (
        <Skeleton rows={3} />
      ) : q.isError ? (
        <ErrorNote error={q.error} />
      ) : list.length === 0 ? (
        <Empty
          icon={<Webhook className="size-5" />}
          title="No channels yet"
          action={can ? create : undefined}
        >
          A channel is where alarm messages are sent: a webhook that receives
          Forge's JSON, a Slack-compatible incoming webhook, or email addresses
          through your SMTP server.{" "}
          {can
            ? "Create one, then add a policy that uses it."
            : CHANNEL_ROLE_HINT}
        </Empty>
      ) : (
        <Table
          head={["Name", "Kind", "Target", "Signed", "Enabled", "Updated", ""]}
        >
          {list.map((c) => {
            const tested = test.variables === c.id && !test.isIdle;
            return (
              <Fragment key={c.id}>
                <tr>
                  <Td
                    className="max-w-[240px] truncate font-medium"
                    title={c.name}
                  >
                    {c.name}
                  </Td>
                  <Td>{KIND_LABEL[c.kind]}</Td>
                  <Td
                    className="max-w-[260px] truncate"
                    title={
                      c.email
                        ? `${c.target} → ${c.email.to.join(", ")}`
                        : c.target || undefined
                    }
                  >
                    <Mono>{c.target || "—"}</Mono>
                  </Td>
                  <Td>
                    {c.kind === "webhook" ? (
                      c.secret_item ? (
                        "yes"
                      ) : (
                        "no"
                      )
                    ) : (
                      <span
                        title={
                          c.kind === "email"
                            ? "Email messages are not signed."
                            : "Slack-compatible messages are not signed; the URL is the secret."
                        }
                      >
                        —
                      </span>
                    )}
                  </Td>
                  <Td>{c.enabled ? "yes" : "no"}</Td>
                  <Td
                    className="text-xs whitespace-nowrap text-muted"
                    title={time(c.updated_at)}
                  >
                    {ago(c.updated_at)}
                  </Td>
                  <Td className="text-right whitespace-nowrap">
                    <IconButton
                      label={
                        !can
                          ? why("Send test")
                          : c.enabled
                            ? "Send test"
                            : "Send test: the channel is turned off"
                      }
                      disabled={!can || !c.enabled || test.isPending}
                      className="disabled:cursor-not-allowed disabled:opacity-50"
                      onClick={() => test.mutate(c.id)}
                    >
                      {test.isPending && test.variables === c.id ? (
                        <Loader2 className="size-4 animate-spin" />
                      ) : (
                        <Send className="size-4" />
                      )}
                    </IconButton>
                    <IconButton
                      label={why("Edit")}
                      disabled={!can}
                      className="disabled:cursor-not-allowed disabled:opacity-50"
                      onClick={() => setEdit(c)}
                    >
                      <Pencil className="size-4" />
                    </IconButton>
                    <IconButton
                      label={why("Delete")}
                      disabled={!can}
                      className="disabled:cursor-not-allowed disabled:opacity-50"
                      onClick={() => {
                        del.reset();
                        setDeleting(c);
                      }}
                    >
                      <Trash2 className="size-4" />
                    </IconButton>
                  </Td>
                </tr>
                {tested && (
                  <tr>
                    <td colSpan={7} className="bg-panel-2 px-4 py-2 text-xs">
                      <TestResult
                        pending={test.isPending}
                        view={test.data}
                        error={test.error}
                        onDismiss={() => test.reset()}
                      />
                    </td>
                  </tr>
                )}
              </Fragment>
            );
          })}
        </Table>
      )}
      {edit && (
        <ChannelDialog
          channel={edit === "new" ? null : edit}
          onClose={() => setEdit(null)}
        />
      )}
      {deleting && (
        <Confirm
          open
          onOpenChange={(o) => !o && setDeleting(null)}
          title={`Delete the channel "${deleting.name}"?`}
          confirmLabel="Delete channel"
          busy={del.isPending}
          disabled={usedBy.length > 0}
          onConfirm={() =>
            del.mutate(deleting.id, {
              onSuccess: () => {
                toast("Channel deleted");
                setDeleting(null);
              },
            })
          }
          body={
            <div className="space-y-2">
              <p>
                No more messages are sent to{" "}
                <Mono>{deleting.target || deleting.name}</Mono>. Messages still
                waiting are cancelled; the delivery history stays.
              </p>
              {usedBy.length > 0 && (
                <p className="text-warn">
                  Used by {usedBy.length === 1 ? "the policy" : "the policies"}{" "}
                  {usedBy.map((p) => `"${p.name}"`).join(", ")}. Remove the
                  channel from {usedBy.length === 1 ? "it" : "them"} first.
                </p>
              )}
              {del.isError && <FormError>{actionError(del.error)}</FormError>}
            </div>
          }
        />
      )}
    </Panel>
  );
}

/** Outcome of **Send test**, shown under the channel's row. */
function TestResult({
  pending,
  view,
  error,
  onDismiss,
}: {
  pending: boolean;
  view: DeliveryView | undefined;
  error: unknown;
  onDismiss: () => void;
}) {
  if (pending)
    return (
      <span className="inline-flex items-center gap-2 text-muted">
        <Loader2 className="size-3.5 animate-spin" /> Sending a test message…
      </span>
    );
  const attempt = view?.attempts[view.attempts.length - 1];
  return (
    <div className="flex flex-wrap items-center gap-2" role="status">
      {error ? (
        <span className="min-w-0 break-words text-bad">
          The test was not sent: {actionError(error)}
        </span>
      ) : view ? (
        <>
          <DeliveryBadge status={view.status} />
          <span className="min-w-0 break-words">
            {view.status === "sent"
              ? "The receiver accepted the test message"
              : view.status === "failed"
                ? "The test message was not delivered"
                : `The test message is ${view.status}`}
            {attempt
              ? ` (${attemptText(attempt)}${attempt.duration_ms !== undefined ? `, ${duration(attempt.duration_ms)}` : ""})`
              : view.last_error || view.last_http_status !== undefined
                ? ` (${attemptText({ http_status: view.last_http_status, error: view.last_error })})`
                : ""}
            {view.detail ? `: ${view.detail}` : "."}
          </span>
        </>
      ) : null}
      <button
        type="button"
        className="ml-auto rounded px-1 text-muted hover:text-fg"
        onClick={onDismiss}
      >
        Dismiss
      </button>
    </div>
  );
}

interface ChannelForm {
  name: string;
  kind: ChannelKind;
  enabled: boolean;
  url: string;
  sign: boolean;
  secret_item: string;
  smtp_host: string;
  /** Empty: the TLS mode's usual port. */
  smtp_port: string;
  tls: SmtpTls;
  username: string;
  /** Empty on edit: keep the stored password. */
  password: string;
  from: string;
  /** One address per line (commas work too). */
  to: string;
  subject_prefix: string;
}

function ChannelDialog({
  channel,
  onClose,
}: {
  channel: Channel | null;
  onClose: () => void;
}) {
  const save = useSaveChannel();
  const toast = useToast();
  const mail = channel?.email;
  const [form, setForm] = useState<ChannelForm>({
    name: channel?.name ?? "",
    kind: channel?.kind ?? "webhook",
    enabled: channel?.enabled ?? true,
    url: "",
    sign: !!channel?.secret_item,
    secret_item: channel?.secret_item ?? "",
    smtp_host: mail?.smtp_host ?? "",
    smtp_port:
      mail && mail.smtp_port !== TLS_PORT[mail.tls]
        ? String(mail.smtp_port)
        : "",
    tls: mail?.tls ?? "starttls",
    username: mail?.username ?? "",
    password: "",
    from: mail?.from ?? "",
    to: mail?.to.join("\n") ?? "",
    subject_prefix: mail?.subject_prefix ?? "",
  });
  const set = (patch: Partial<ChannelForm>) =>
    setForm((f) => ({ ...f, ...patch }));
  const isEmail = form.kind === "email";
  const signing = form.kind === "webhook" && form.sign;
  const url = form.url.trim();
  const recipients = splitAddresses(form.to);
  const port = form.smtp_port.trim();
  const username = form.username.trim();
  // The stored password stays while the user name is kept on an email channel.
  const passwordStored = channel?.kind === "email" && !!mail?.password_set;

  const urlProblem = isEmail
    ? undefined
    : (!channel || channel.kind === "email") && !url
      ? "Enter the URL messages are sent to."
      : url && !/^https?:\/\/\S+$/i.test(url)
        ? "The URL starts with https:// and has no spaces."
        : new TextEncoder().encode(url).length > NOTIFY_LIMITS.url
          ? `The URL is at most ${NOTIFY_LIMITS.url} bytes.`
          : signing && !form.secret_item
            ? "Choose a signing secret, or turn signing off."
            : undefined;
  const mailProblem = !isEmail
    ? undefined
    : !form.smtp_host.trim()
      ? "Enter the SMTP server."
      : port &&
          !(/^\d+$/.test(port) && Number(port) >= 1 && Number(port) <= 65535)
        ? "The port is a number from 1 to 65535."
        : form.tls === "none" && username
          ? "A password is never sent without TLS: choose STARTTLS or implicit TLS, or remove the user name."
          : username && !form.password && !passwordStored
            ? "Enter the SMTP password for this user name."
            : !looksLikeAddress(form.from.trim())
              ? "Enter the sender address."
              : recipients.length === 0
                ? "Enter at least one recipient."
                : recipients.length > NOTIFY_LIMITS.recipients
                  ? `At most ${NOTIFY_LIMITS.recipients} recipients.`
                  : recipients.find((a) => !looksLikeAddress(a))
                    ? `"${recipients.find((a) => !looksLikeAddress(a))}" is not an email address.`
                    : form.subject_prefix.trim().length >
                        NOTIFY_LIMITS.subjectPrefix
                      ? `The subject prefix is at most ${NOTIFY_LIMITS.subjectPrefix} characters.`
                      : undefined;
  const problem = !form.name.trim()
    ? "Name the channel."
    : (urlProblem ?? mailProblem);

  const body: ChannelBody = isEmail
    ? {
        name: form.name.trim(),
        kind: "email",
        enabled: form.enabled,
        email: {
          smtp_host: form.smtp_host.trim(),
          smtp_port: port ? Number(port) : undefined,
          tls: form.tls,
          username: username || undefined,
          password: username && form.password ? form.password : undefined,
          from: form.from.trim(),
          to: recipients,
          subject_prefix: form.subject_prefix.trim() || undefined,
        },
      }
    : {
        name: form.name.trim(),
        kind: form.kind,
        enabled: form.enabled,
        url: url || undefined,
        secret_item: signing && form.secret_item ? form.secret_item : undefined,
      };

  return (
    <Modal
      open
      onOpenChange={(o) => !o && onClose()}
      title={channel ? "Edit channel" : "New channel"}
      description={
        isEmail
          ? "The SMTP password is stored encrypted and is never shown again."
          : "The URL is stored encrypted. Afterwards the console shows only its host, never the full URL."
      }
    >
      <form
        className="space-y-3"
        onSubmit={(e) => {
          e.preventDefault();
          if (problem) return;
          save.mutate(
            { id: channel?.id, body },
            {
              onSuccess: () => {
                toast(channel ? "Channel saved" : "Channel created");
                onClose();
              },
            },
          );
        }}
      >
        <Field label="Name">
          <Input
            value={form.name}
            maxLength={NOTIFY_LIMITS.name}
            onChange={(e) => set({ name: e.target.value })}
            autoFocus
            placeholder="Ops webhook"
          />
        </Field>
        <Field
          label="Kind"
          hint={
            form.kind === "webhook"
              ? "Sends Forge's JSON payload with a POST request; can be signed."
              : form.kind === "slack"
                ? "Sends a text message to a Slack incoming webhook or a compatible receiver (Mattermost, Discord's /slack endpoint). Not signed: the URL is the secret."
                : "Sends a plain-text email through your SMTP server to up to 20 recipients."
          }
        >
          <Select
            value={form.kind}
            onChange={(e) => set({ kind: e.target.value as ChannelKind })}
          >
            <option value="webhook">Webhook</option>
            <option value="slack">Slack-compatible</option>
            <option value="email">Email</option>
          </Select>
        </Field>
        {isEmail ? (
          <EmailFields form={form} set={set} passwordStored={passwordStored} />
        ) : (
          <Field
            label="URL"
            hint={
              channel ? (
                <>
                  Current target: <Mono>{channel.target || "—"}</Mono>. Leave
                  empty to keep the stored URL.
                </>
              ) : (
                "https://host/path. Private, loopback and metadata addresses are refused unless the server configuration allows them."
              )
            }
          >
            <Input
              value={form.url}
              onChange={(e) => set({ url: e.target.value })}
              placeholder={
                channel ? "Unchanged" : "https://hooks.example.com/…"
              }
              autoComplete="off"
              spellCheck={false}
              inputMode="url"
              className="font-mono"
            />
          </Field>
        )}
        {form.kind === "webhook" && (
          <div className="space-y-2">
            <Switch
              checked={form.sign}
              onChange={(sign) => set({ sign })}
              label="Sign requests"
            />
            {form.sign ? (
              <SecretPicker
                value={form.secret_item}
                current={channel?.secret_item}
                onChange={(secret_item) => set({ secret_item })}
              />
            ) : (
              <p className="text-xs text-faint">
                Optional. With a signing secret from the credential vault, each
                request carries an HMAC-SHA256 signature the receiver can
                verify.
              </p>
            )}
          </div>
        )}
        <Switch
          checked={form.enabled}
          onChange={(enabled) => set({ enabled })}
          label="Enabled"
        />
        <p className="text-xs text-faint">
          Saving a channel needs an MFA-verified sign-in; you may be asked for a
          code.
        </p>
        {problem && <p className="text-xs text-bad">{problem}</p>}
        {save.isError && <FormError>{actionError(save.error)}</FormError>}
        <div className="flex justify-end gap-2">
          <Button type="button" variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" disabled={save.isPending || !!problem}>
            {save.isPending && <Loader2 className="size-4 animate-spin" />}
            Save
          </Button>
        </div>
      </form>
    </Modal>
  );
}

/** The SMTP settings of an email channel. The password is write-only. */
function EmailFields({
  form,
  set,
  passwordStored,
}: {
  form: ChannelForm;
  set: (patch: Partial<ChannelForm>) => void;
  passwordStored: boolean;
}) {
  const hasUser = !!form.username.trim();
  return (
    <div className="space-y-3">
      <div className="grid gap-3 sm:grid-cols-[1fr_110px]">
        <Field
          label="SMTP server"
          hint="Host name or IP address. Private, loopback and metadata addresses are refused unless the server configuration allows them."
        >
          <Input
            value={form.smtp_host}
            maxLength={NOTIFY_LIMITS.smtpHost}
            onChange={(e) => set({ smtp_host: e.target.value })}
            placeholder="smtp.example.com"
            autoComplete="off"
            spellCheck={false}
            className="font-mono"
          />
        </Field>
        <Field label="Port">
          <Input
            value={form.smtp_port}
            onChange={(e) => set({ smtp_port: e.target.value })}
            placeholder={String(TLS_PORT[form.tls])}
            inputMode="numeric"
            className="font-mono"
          />
        </Field>
      </div>
      <Field
        label="Encryption"
        hint={
          form.tls === "none"
            ? "Messages travel unencrypted. Allowed only where the server configuration allows unencrypted targets, and never with a user name."
            : "The certificate is verified against the host name."
        }
      >
        <Select
          value={form.tls}
          onChange={(e) => set({ tls: e.target.value as SmtpTls })}
        >
          {(Object.keys(TLS_LABEL) as SmtpTls[]).map((t) => (
            <option key={t} value={t}>
              {TLS_LABEL[t]}
            </option>
          ))}
        </Select>
      </Field>
      <div className="grid gap-3 sm:grid-cols-2">
        <Field
          label="User name"
          hint="Leave empty to send without authentication."
        >
          <Input
            value={form.username}
            onChange={(e) => set({ username: e.target.value })}
            autoComplete="off"
            spellCheck={false}
          />
        </Field>
        <Field
          label={passwordStored && hasUser ? "Replace password" : "Password"}
          hint={
            passwordStored && hasUser
              ? "A password is stored. Leave empty to keep it."
              : hasUser
                ? "Stored encrypted; never shown again."
                : "Needs a user name."
          }
        >
          <Input
            type="password"
            value={form.password}
            disabled={!hasUser}
            onChange={(e) => set({ password: e.target.value })}
            placeholder={passwordStored && hasUser ? "Unchanged" : ""}
            autoComplete="new-password"
          />
        </Field>
      </div>
      <Field
        label="From"
        hint="forge@example.com or Forge <forge@example.com>. Your SMTP server may require an address it owns."
      >
        <Input
          value={form.from}
          onChange={(e) => set({ from: e.target.value })}
          placeholder="Forge <forge@example.com>"
          autoComplete="off"
          spellCheck={false}
        />
      </Field>
      <Field
        label="To"
        hint={`One address per line, up to ${NOTIFY_LIMITS.recipients}.`}
      >
        <TextArea
          rows={3}
          value={form.to}
          onChange={(e) => set({ to: e.target.value })}
          placeholder={"ops@example.com\noncall@example.com"}
          spellCheck={false}
        />
      </Field>
      <Field
        label="Subject prefix"
        hint="Optional, put before every subject, for example [prod]."
      >
        <Input
          value={form.subject_prefix}
          maxLength={NOTIFY_LIMITS.subjectPrefix}
          onChange={(e) => set({ subject_prefix: e.target.value })}
          placeholder="[prod]"
        />
      </Field>
    </div>
  );
}

/**
 * Picks the vault item whose password signs the requests. The vault list is
 * only open to platform administrators, so a refused request is explained
 * here instead of looking like an empty vault.
 */
function SecretPicker({
  value,
  current,
  onChange,
}: {
  value: string;
  current?: string;
  onChange: (id: string) => void;
}) {
  const items = useVaultItems();
  const list = Array.isArray(items.data) ? items.data : [];
  if (items.isLoading) return <Skeleton rows={1} />;
  if (items.isError) {
    const refused =
      items.error instanceof ApiError && items.error.status === 403;
    return (
      <div className="space-y-2 rounded-md border border-line px-3 py-2 text-xs">
        <p className="text-warn">
          {refused
            ? "The credential vault can only be listed by a platform administrator with an MFA-verified sign-in, so a signing secret cannot be chosen from this account."
            : `The credential vault could not be loaded: ${errorText(items.error)}`}
        </p>
        {refused && (
          <p className="text-muted">
            Server answer: {actionError(items.error)}
          </p>
        )}
        {current ? (
          <Switch
            checked={value === current}
            onChange={(keep) => onChange(keep ? current : "")}
            label={`Keep the current signing secret (vault item ${short(current)})`}
          />
        ) : (
          <p className="text-muted">
            Turn signing off to save this channel unsigned, or ask a platform
            administrator to set the secret.
          </p>
        )}
        <Button
          type="button"
          onClick={() => void items.refetch()}
          disabled={items.isFetching}
        >
          {items.isFetching && <Loader2 className="size-4 animate-spin" />}
          Try again
        </Button>
      </div>
    );
  }
  if (list.length === 0 && !current)
    return (
      <p className="rounded-md border border-line px-3 py-2 text-xs text-muted">
        The credential vault has no items. Add one under{" "}
        <Link to="/vault" className="text-accent hover:underline">
          Credential vault
        </Link>{" "}
        first; its password becomes the signing key.
      </p>
    );
  return (
    <Field
      label="Signing secret"
      hint="The item's password is the HMAC key. Forge reads it when it sends; it is never shown here."
    >
      <Select value={value} onChange={(e) => onChange(e.target.value)}>
        <option value="">Choose a vault item…</option>
        {current && !list.some((i) => i.id === current) && (
          <option value={current}>
            Current item ({short(current)}, not in the list)
          </option>
        )}
        {list.map((i) => (
          <option key={i.id} value={i.id}>
            {i.name}
          </option>
        ))}
      </Select>
    </Field>
  );
}
