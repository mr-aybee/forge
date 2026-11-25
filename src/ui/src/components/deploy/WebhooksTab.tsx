// The Webhooks tab of a service or stack (D-159): secret deploy URLs CI calls.
import { Link } from "@tanstack/react-router";
import { AlertTriangle, History, Loader2, Pencil, Plus, RotateCw, Trash2, Webhook as WebhookIcon } from "lucide-react";
import { useState } from "react";

import { ApiError } from "../../api/client";
import {
  WEBHOOK_SIGNATURES,
  type DeliveryOutcome,
  type Webhook,
  type WebhookIssued,
  type WebhookSignature,
  type WebhookTarget,
  useCreateWebhook,
  useDeleteWebhook,
  useRotateWebhook,
  useUpdateWebhook,
  useWebhookDeliveries,
  useWebhooks,
  webhookUrl,
} from "../../api/webhooks";
import { errorText, useToast } from "../../app/toast";
import { ago, Badge, Button, Confirm, CopyButton, CopyIcon, Drawer, Empty, ErrorNote, Field, Input, Modal, Panel, RowMenu, type RowAction, Select, Skeleton, Switch, Table, Td, time, useRowMenu } from "../ui";

const SIG: Record<WebhookSignature, { label: string; tone: "ok" | "warn" | "muted"; help: string }> = {
  forge: { label: "Forge", tone: "ok", help: "HMAC of timestamp and body (X-Forge-Signature). Best for any CI." },
  github: { label: "GitHub", tone: "ok", help: "HMAC of the body in X-Hub-Signature-256, as GitHub signs deliveries." },
  gitlab: { label: "GitLab", tone: "ok", help: "The secret in X-Gitlab-Token and a unique X-Gitlab-Event-UUID per delivery, as GitLab sends them." },
  none: { label: "None", tone: "warn", help: "The URL alone. Use it only on a trusted network." },
};

const OUTCOME: Record<DeliveryOutcome, "ok" | "warn" | "bad"> = { accepted: "ok", rejected: "bad", rate_limited: "warn", failed: "bad" };

type Dialog = { k: "create" } | { k: "edit"; w: Webhook } | { k: "rotate"; w: Webhook } | { k: "delete"; w: Webhook } | { k: "deliveries"; w: Webhook } | { k: "issued"; issued: WebhookIssued; rotated: boolean };

/** Webhooks that deploy `target` (a service or a stack). */
export function WebhooksTab({ target, targetName }: { target: WebhookTarget; targetName: string }) {
  const q = useWebhooks(target);
  const update = useUpdateWebhook();
  const rotate = useRotateWebhook();
  const del = useDeleteWebhook();
  const toast = useToast();
  const rm = useRowMenu();
  const [dialog, setDialog] = useState<Dialog | null>(null);
  const [toggling, setToggling] = useState<string | null>(null);
  const close = () => setDialog(null);
  const missing = q.error instanceof ApiError && (q.error.status === 404 || q.error.status === 405);
  const denied = q.error instanceof ApiError && q.error.status === 403;
  const rows = q.data ?? [];
  const what = target.kind === "service" ? "service" : "stack";

  const setEnabled = (w: Webhook, enabled: boolean) => {
    setToggling(w.id);
    update.mutate(
      { id: w.id, body: { name: w.name, target: w.target, signature: w.signature, tag_pattern: w.tag_pattern, enabled } },
      {
        onSuccess: () => toast(`${enabled ? "Enabled" : "Disabled"} ${w.name}`),
        onError: (e) => toast(errorText(e), "error"),
        onSettled: () => setToggling(null),
      },
    );
  };

  return (
    <Panel
      title="Deploy webhooks"
      description={`Secret URLs CI calls to deploy this ${what}. Each delivery runs with its creator's permissions.`}
      actions={
        <Button variant="primary" onClick={() => setDialog({ k: "create" })} disabled={missing || denied} title={missing ? "This Forge server does not offer webhooks yet" : undefined}>
          <Plus className="size-4" /> Create webhook
        </Button>
      }
    >
      {denied ? (
        <p className="p-4 text-[13px] text-muted">You cannot see the webhooks of this workspace.</p>
      ) : missing ? (
        <ErrorNote error="This Forge server does not offer deploy webhooks yet. Update Forge to use them." />
      ) : q.error ? (
        <ErrorNote error={q.error} />
      ) : q.isLoading ? (
        <Skeleton rows={3} />
      ) : rows.length === 0 ? (
        <Empty
          icon={<WebhookIcon className="size-5" />}
          title="No webhooks"
          action={
            <Button variant="primary" onClick={() => setDialog({ k: "create" })}>
              <Plus className="size-4" /> Create webhook
            </Button>
          }
        >
          Create one to deploy {targetName || `this ${what}`} from GitHub Actions, GitLab CI or any script.
        </Empty>
      ) : (
        <Table head={["Name", "Signature", "URL", "Tag pattern", "Enabled", "Last delivery", ""]}>
          {rows.map((w) => {
            const actions: RowAction[] = [
              { label: "Deliveries", icon: History, tone: "accent", onSelect: () => setDialog({ k: "deliveries", w }) },
              { label: "Edit…", icon: Pencil, tone: "accent", onSelect: () => setDialog({ k: "edit", w }) },
              { label: "Rotate…", icon: RotateCw, tone: "warn", onSelect: () => setDialog({ k: "rotate", w }) },
              { label: "Delete…", icon: Trash2, danger: true, onSelect: () => setDialog({ k: "delete", w }) },
            ];
            const sig = SIG[w.signature];
            return (
              <tr key={w.id} className="hover:bg-panel-2/60" {...rm.rowProps(actions, `Actions for ${w.name}`)}>
                <Td className="max-w-[220px]">
                  <button type="button" className="block max-w-full truncate text-left text-[13px] font-medium hover:text-accent focus-visible:outline-2 focus-visible:outline-accent" title={w.name} onClick={() => setDialog({ k: "deliveries", w })}>
                    {w.name || "—"}
                  </button>
                </Td>
                <Td className="whitespace-nowrap" title={sig.help}>
                  <Badge tone={sig.tone}>{sig.label}</Badge>
                </Td>
                <Td className="max-w-[220px] truncate font-mono text-[11px] text-muted" title="Only the end of the token is kept for display">
                  {w.token_hint ? `/hooks/…${w.token_hint}` : "—"}
                </Td>
                <Td className="max-w-[160px] truncate font-mono text-[11px]" title={w.tag_pattern ?? "No tag changes allowed"}>
                  {w.tag_pattern ?? <span className="text-faint">—</span>}
                </Td>
                <Td className="whitespace-nowrap">
                  <span className="inline-flex items-center gap-1.5">
                    <Switch checked={w.enabled} onChange={(v) => setEnabled(w, v)} disabled={toggling === w.id} label={<span className="sr-only">Enabled</span>} title={w.enabled ? "Disable" : "Enable"} />
                    {toggling === w.id && <Loader2 className="size-3.5 animate-spin text-muted" />}
                  </span>
                </Td>
                <Td className="text-xs whitespace-nowrap text-muted" title={w.last_delivery_at ? time(w.last_delivery_at) : undefined}>
                  {w.last_delivery_at ? ago(w.last_delivery_at) : "—"}
                </Td>
                <Td className="text-right">
                  <RowMenu actions={actions} label={`Actions for ${w.name}`} />
                </Td>
              </tr>
            );
          })}
        </Table>
      )}
      {rm.menu}

      {dialog?.k === "create" && <WebhookDialog target={target} onClose={close} onIssued={(issued) => setDialog({ k: "issued", issued, rotated: false })} />}
      {dialog?.k === "edit" && <WebhookDialog target={target} existing={dialog.w} onClose={close} />}
      {dialog?.k === "issued" && <IssuedDialog issued={dialog.issued} rotated={dialog.rotated} onClose={close} />}
      {dialog?.k === "deliveries" && <DeliveriesDrawer w={dialog.w} onClose={close} />}
      {dialog?.k === "rotate" && (
        <Confirm
          open
          onOpenChange={(o) => !o && close()}
          title={`Rotate ${dialog.w.name}?`}
          body={
            <>
              Forge issues a new URL{dialog.w.signature === "none" ? "" : " and secret"}. The old URL stops working at once: CI jobs that call it get{" "}
              <span className="font-mono">404</span> until you update their secrets.
            </>
          }
          confirmLabel="Rotate"
          busy={rotate.isPending}
          onConfirm={() =>
            rotate.mutate(dialog.w.id, {
              onSuccess: (issued) => setDialog({ k: "issued", issued, rotated: true }),
              onError: (e) => toast(errorText(e), "error"),
            })
          }
        />
      )}
      {dialog?.k === "delete" && (
        <Confirm
          open
          onOpenChange={(o) => !o && close()}
          title={`Delete ${dialog.w.name}?`}
          body={<>The URL stops working at once and its delivery history is removed. CI jobs that call it fail with <span className="font-mono">404</span>.</>}
          confirmLabel="Delete"
          busy={del.isPending}
          onConfirm={() =>
            del.mutate(dialog.w.id, {
              onSuccess: () => {
                toast(`Deleted ${dialog.w.name}`);
                close();
              },
              onError: (e) => toast(errorText(e), "error"),
            })
          }
        />
      )}
    </Panel>
  );
}

function WebhookDialog({ target, existing, onClose, onIssued }: { target: WebhookTarget; existing?: Webhook; onClose: () => void; onIssued?: (i: WebhookIssued) => void }) {
  const create = useCreateWebhook();
  const update = useUpdateWebhook();
  const toast = useToast();
  const [form, setForm] = useState({
    name: existing?.name ?? "",
    signature: existing?.signature ?? ("forge" as WebhookSignature),
    tag_pattern: existing?.tag_pattern ?? "",
    enabled: existing?.enabled ?? true,
  });
  const [error, setError] = useState<unknown>();
  const busy = create.isPending || update.isPending;
  const set = (p: Partial<typeof form>) => setForm((f) => ({ ...f, ...p }));
  const valid = form.name.trim().length > 0;

  const submit = () => {
    setError(undefined);
    const body = { name: form.name.trim(), target, signature: form.signature, tag_pattern: form.tag_pattern.trim() || null, enabled: form.enabled };
    if (existing) {
      update.mutate(
        { id: existing.id, body },
        {
          onSuccess: () => {
            toast(`Saved ${body.name}`);
            onClose();
          },
          onError: setError,
        },
      );
    } else {
      create.mutate(body, { onSuccess: (issued) => onIssued?.(issued), onError: setError });
    }
  };

  return (
    <Modal open onOpenChange={(o) => !o && onClose()} title={existing ? `Edit ${existing.name}` : "Create webhook"} description="A secret URL that deploys this target when CI calls it.">
      <form
        className="space-y-3"
        onSubmit={(e) => {
          e.preventDefault();
          if (valid && !busy) submit();
        }}
      >
        <Field label="Name">
          <Input value={form.name} onChange={(e) => set({ name: e.target.value })} placeholder="e.g. GitHub release" maxLength={80} required autoFocus />
        </Field>
        <Field label="Signature" hint={existing ? "The signature cannot change. Create a new webhook to use another." : SIG[form.signature].help}>
          <Select value={form.signature} onChange={(e) => set({ signature: e.target.value as WebhookSignature })} disabled={!!existing}>
            {WEBHOOK_SIGNATURES.map((s) => (
              <option key={s} value={s}>
                {SIG[s].label}
              </option>
            ))}
          </Select>
        </Field>
        <Field
          label="Tag pattern"
          hint={
            <>
              Optional. A delivery may deploy an image tag that matches, e.g. <span className="font-mono">v*</span> (<span className="font-mono">*</span> matches anything). Empty: the tag cannot change.
            </>
          }
        >
          <Input className="font-mono" value={form.tag_pattern} onChange={(e) => set({ tag_pattern: e.target.value })} placeholder="v*" maxLength={128} spellCheck={false} />
        </Field>
        <Switch checked={form.enabled} onChange={(enabled) => set({ enabled })} label="Enabled" />
        {error !== undefined && <p className="rounded-md bg-bad-soft px-3 py-2 text-xs text-bad">{errorText(error)}</p>}
        <div className="flex justify-end gap-2 pt-1">
          <Button type="button" onClick={onClose}>
            Cancel
          </Button>
          <Button type="submit" variant="primary" disabled={!valid || busy}>
            {busy && <Loader2 className="size-4 animate-spin" />}
            {existing ? "Save" : "Create"}
          </Button>
        </div>
      </form>
    </Modal>
  );
}

type SnippetKind = "github" | "gitlab" | "curl";

/** Shell lines that sign `$body` with `$<secretVar>` and the curl header flags for `sig`. */
function signing(sig: WebhookSignature, secretVar: string): { lines: string[]; headers: string } {
  switch (sig) {
    case "forge":
      return {
        lines: ["ts=$(date +%s)", `sig=$(printf '%s.%s' "$ts" "$body" | openssl dgst -sha256 -hmac "$${secretVar}" -hex | sed 's/^.* //')`],
        headers: ` -H "X-Forge-Timestamp: $ts" -H "X-Forge-Signature: sha256=$sig"`,
      };
    case "github":
      return {
        lines: [`sig=$(printf '%s' "$body" | openssl dgst -sha256 -hmac "$${secretVar}" -hex | sed 's/^.* //')`],
        headers: ` -H "X-Hub-Signature-256: sha256=$sig"`,
      };
    case "gitlab":
      // Each delivery needs its own id: a repeated one is refused as a replay.
      return {
        lines: ["id=$(cat /proc/sys/kernel/random/uuid)"],
        headers: ` -H "X-Gitlab-Token: $${secretVar}" -H "X-Gitlab-Event-UUID: $id"`,
      };
    case "none":
      return { lines: [], headers: "" };
  }
}

/** A ready-to-paste snippet; never contains the real secret. Only curl embeds the URL. */
function snippet(kind: SnippetKind, sig: WebhookSignature, withTag: boolean, url: string): string {
  const tagExpr = kind === "github" ? "${GITHUB_REF_NAME}" : kind === "gitlab" ? "${CI_COMMIT_TAG}" : "v1.4.0";
  const body = withTag ? `body="{\\"tag\\":\\"${tagExpr}\\"}"` : "body='{}'";
  const urlVar = kind === "github" ? "URL" : "FORGE_WEBHOOK_URL";
  const secretVar = kind === "github" ? "SECRET" : "FORGE_WEBHOOK_SECRET";
  const s = signing(sig, secretVar);
  const target = kind === "curl" ? `"${url}"` : `"$${urlVar}"`;
  const call = `curl -fsS -X POST ${target} -H "Content-Type: application/json"${s.headers} -d "$body"`;
  const lines = [body, ...s.lines, call];
  if (kind === "github") {
    return [
      "- name: Deploy",
      "  env:",
      "    URL: ${{ secrets.FORGE_WEBHOOK_URL }}",
      ...(sig === "none" ? [] : ["    SECRET: ${{ secrets.FORGE_WEBHOOK_SECRET }}"]),
      "  run: |",
      ...lines.map((l) => `    ${l}`),
    ].join("\n");
  }
  if (kind === "gitlab") {
    return ["deploy:", ...(withTag ? ["  rules:", "    - if: $CI_COMMIT_TAG"] : []), "  script:", "    - |", ...lines.map((l) => `      ${l}`)].join("\n");
  }
  const pre = sig === "none" ? [] : ["# export FORGE_WEBHOOK_SECRET=<secret>"];
  return [...pre, ...lines].join("\n");
}

function IssuedDialog({ issued, rotated, onClose }: { issued: WebhookIssued; rotated: boolean; onClose: () => void }) {
  const [kind, setKind] = useState<SnippetKind>("github");
  const url = issued.token ? webhookUrl(issued.token) : "";
  const w = issued.webhook;
  const code = snippet(kind, w.signature, !!w.tag_pattern, url || "<url>");
  const vars = kind === "github" ? "repository secrets FORGE_WEBHOOK_URL and FORGE_WEBHOOK_SECRET" : kind === "gitlab" ? "masked CI/CD variables FORGE_WEBHOOK_URL and FORGE_WEBHOOK_SECRET" : "the FORGE_WEBHOOK_SECRET environment variable";

  return (
    <Modal open onOpenChange={(o) => !o && onClose()} title={rotated ? `Rotated ${w.name}` : `Created ${w.name}`} description="Copy the URL and secret now. They are shown only once." wide>
      <div className="space-y-3">
        <div className="flex items-start gap-2 rounded-md bg-warn-soft px-3 py-2 text-xs text-warn">
          <AlertTriangle className="mt-px size-3.5 shrink-0" />
          <span>
            Shown only once: Forge keeps a hash, not the value. Treat the URL as a password; anyone with it{w.signature === "none" ? "" : " and the secret"} can deploy.
            {rotated && " The old URL has stopped working."}
          </span>
        </div>
        <Secret label="URL" value={url} />
        {issued.secret ? <Secret label="Secret" value={issued.secret} /> : <p className="text-xs text-muted">No secret: this webhook uses no signature.</p>}
        <div>
          <div className="mb-1 flex items-center justify-between gap-2">
            <div className="flex gap-1" role="tablist" aria-label="Snippet">
              {(
                [
                  ["github", "GitHub Actions"],
                  ["gitlab", "GitLab CI"],
                  ["curl", "curl"],
                ] as const
              ).map(([id, label]) => (
                <button key={id} type="button" role="tab" aria-selected={kind === id} onClick={() => setKind(id)} className={`press rounded px-2 py-1 text-xs focus-visible:outline-2 focus-visible:outline-accent ${kind === id ? "bg-accent-soft text-accent" : "text-muted hover:text-fg"}`}>
                  {label}
                </button>
              ))}
            </div>
            <CopyIcon text={code} label="Copy snippet" />
          </div>
          <pre className="max-h-64 overflow-auto rounded-md border border-line bg-panel-2 p-2.5 font-mono text-[11px] leading-relaxed whitespace-pre select-all">{code}</pre>
          <p className="mt-1 text-[11px] text-faint">Store the values as {vars}.{w.tag_pattern ? ` The tag must match ${w.tag_pattern}.` : ""}</p>
        </div>
        <div className="flex justify-end">
          <Button variant="primary" onClick={onClose}>
            I have copied them
          </Button>
        </div>
      </div>
    </Modal>
  );
}

function Secret({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <div className="mb-1 text-xs font-medium text-muted">{label}</div>
      <div className="flex min-w-0 items-center gap-2">
        <code className="min-w-0 flex-1 truncate rounded-md border border-line bg-panel-2 px-2.5 py-1.5 font-mono text-[12px] select-all" title={value}>
          {value || "—"}
        </code>
        {value && <CopyButton text={value} />}
      </div>
    </div>
  );
}

function DeliveriesDrawer({ w, onClose }: { w: Webhook; onClose: () => void }) {
  const q = useWebhookDeliveries(w.id);
  const rows = q.data ?? [];
  return (
    <Drawer open onOpenChange={(o) => !o && onClose()} title={`Deliveries · ${w.name}`} wide>
      <p className="mb-3 text-xs text-muted">The newest 100 calls to this webhook. Refreshes every 10 seconds.</p>
      <div className="overflow-hidden rounded-md border border-line">
        {q.error ? (
          <ErrorNote error={q.error} />
        ) : q.isLoading ? (
          <Skeleton rows={4} />
        ) : rows.length === 0 ? (
          <Empty icon={<History className="size-5" />} title="No deliveries yet">
            Call the URL from CI; each call shows up here with its outcome.
          </Empty>
        ) : (
          <Table head={["When", "Outcome", "Reason", "Tag", "Source", "Job"]}>
            {rows.map((d) => (
              <tr key={d.id}>
                <Td className="text-xs whitespace-nowrap text-muted" title={d.at ? time(d.at) : undefined}>
                  {d.at ? ago(d.at) : "—"}
                </Td>
                <Td className="whitespace-nowrap">
                  <Badge tone={OUTCOME[d.outcome]}>{d.outcome.replace("_", " ")}</Badge>
                </Td>
                <Td className="max-w-[240px] truncate text-xs" title={d.reason}>
                  {d.reason || "—"}
                </Td>
                <Td className="max-w-[120px] truncate font-mono text-[11px]" title={d.tag ?? undefined}>
                  {d.tag ?? "—"}
                </Td>
                <Td className="font-mono text-[11px] whitespace-nowrap text-muted select-all">{d.source_ip || "—"}</Td>
                <Td className="whitespace-nowrap">
                  {d.job_id ? (
                    <span className="inline-flex items-center gap-1">
                      <Link to="/jobs" className="font-mono text-[11px] text-accent hover:underline" title={`Job ${d.job_id}`}>
                        {d.job_id.slice(0, 8)}
                      </Link>
                      <CopyIcon text={d.job_id} label="Copy job id" />
                    </span>
                  ) : (
                    "—"
                  )}
                </Td>
              </tr>
            ))}
          </Table>
        )}
      </div>
    </Drawer>
  );
}
