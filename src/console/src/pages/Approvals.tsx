import { Link, useNavigate, useSearch } from "@tanstack/react-router";
import { ArrowRight, Bot, ChevronRight, ClipboardCheck, Loader2, ShieldAlert, ShieldCheck, Timer, TriangleAlert } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";

import { ApiError } from "../api/client";
import { actionTitle, effectiveStatus, useApproval, useApprovals, useDecideApproval, type Approval } from "../api/approvals";
import { useMe } from "../api/hooks";
import { errorText, useToast } from "../app/toast";
import { Badge, Button, CopyIcon, Empty, ErrorNote, Modal, PageHeader, Panel, Skeleton, Status, Table, Td, ago, time } from "../components/ui";

/** Re-renders every `every` ms so countdowns stay current between polls. */
function useNow(every: number) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), every);
    return () => clearInterval(t);
  }, [every]);
  return now;
}

/** "4m 12s", "38s"; `—` when unknown. */
function left(until: number | null, now: number): string {
  if (until === null) return "—";
  const s = Math.max(0, Math.round((until - now) / 1000));
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m ${String(s % 60).padStart(2, "0")}s`;
  return `${Math.floor(s / 3600)}h ${String(Math.floor((s % 3600) / 60)).padStart(2, "0")}m`;
}

/** Pretty JSON for display; never throws. */
function pretty(v: unknown): string {
  if (v === null || v === undefined) return "—";
  if (typeof v === "string") return v;
  try {
    return JSON.stringify(v, null, 2) ?? "—";
  } catch {
    return String(v);
  }
}

const isEmptyJson = (v: unknown) => v === null || v === undefined || (typeof v === "object" && !Array.isArray(v) && Object.keys(v as object).length === 0);

/**
 * Approvals (MCP Ask mode, issue #6): changes your AI connectors asked for.
 * Nothing runs until you approve it here; approving runs the request once,
 * as you. `?id=` (the link the agent sends) opens one approval directly.
 */
export function Approvals() {
  const { id } = useSearch({ from: "/approvals" });
  const navigate = useNavigate();
  const q = useApprovals();
  const now = useNow(5_000);
  const r = q.data;
  const all = r?.approvals ?? [];
  const pending = all.filter((a) => effectiveStatus(a, now) === "pending").sort((a, b) => (a.expires_at ?? 0) - (b.expires_at ?? 0));
  const recent = all.filter((a) => effectiveStatus(a, now) !== "pending");
  const open = (to: string | undefined) => void navigate({ to: "/approvals", search: to ? { id: to } : {}, replace: true });

  return (
    <div>
      <PageHeader
        title="Approvals"
        description="Changes your AI connectors asked for in Ask mode. Nothing runs until you approve it here; then it runs once, as you, with your permissions."
      />

      {q.isLoading ? (
        <Panel>
          <Skeleton rows={4} />
        </Panel>
      ) : r?.status === "off" ? (
        <Panel>
          <Empty icon={<ClipboardCheck className="size-5" />} title="Approvals are not available">
            This Forge server does not support approvals yet. Update the server to use Ask mode for AI connectors.
          </Empty>
        </Panel>
      ) : r && r.status !== "ok" ? (
        <Panel>
          <ErrorNote error={r.error || "Could not load approvals."} />
        </Panel>
      ) : (
        <div className="space-y-4">
          <Panel title="Waiting for you" description={pending.length ? `${pending.length} pending` : undefined}>
            {pending.length === 0 ? (
              <Empty icon={<ShieldCheck className="size-5" />} title="Nothing waiting">
                When a connector in <strong>Ask</strong> mode wants to change something, the request appears here. Manage connectors in{" "}
                <Link to="/profile/$section" params={{ section: "tokens" }} className="text-accent hover:underline">
                  Profile → Tokens &amp; API keys
                </Link>
                .
              </Empty>
            ) : (
              <ul className="divide-y divide-line">
                {pending.map((a) => (
                  <li key={a.id}>
                    <button
                      onClick={() => open(a.id)}
                      className="flex w-full min-w-0 items-center gap-3 px-4 py-2 text-left text-[13px] transition hover:bg-panel-2 focus-visible:bg-panel-2"
                    >
                      <Bot className="size-3.5 shrink-0 text-muted" />
                      <span className="flex min-w-0 flex-1 flex-col gap-0.5 sm:flex-row sm:items-center sm:gap-2">
                        <span className="shrink-0 truncate text-muted sm:max-w-[30%]" title={a.requester}>
                          {a.requester}
                        </span>
                        <ArrowRight className="hidden size-3.5 shrink-0 text-faint sm:block" />
                        <span className="shrink-0 font-medium">{actionTitle(a.action)}</span>
                        <span className="min-w-0 truncate text-xs text-muted" title={a.summary}>
                          {a.summary || "—"}
                        </span>
                      </span>
                      {a.mfa_required && (
                        <span title="Needs an MFA-verified session" className="shrink-0 text-warn">
                          <ShieldAlert className="size-3.5" />
                        </span>
                      )}
                      <span className="flex shrink-0 items-center gap-1 font-mono text-[11px] text-muted tabular-nums" title={`Expires ${time(a.expires_at)}`}>
                        <Timer className="size-3.5" /> {left(a.expires_at, now)}
                      </span>
                      <ChevronRight className="size-3.5 shrink-0 text-faint" />
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </Panel>

          <Panel title="Recent" description="Decided and expired requests, newest first.">
            {recent.length === 0 ? (
              <p className="px-4 py-3 text-xs text-muted">No decided requests yet.</p>
            ) : (
              <Table head={["Status", "Connector", "Action", "Request", "Decided"]}>
                {recent.map((a) => (
                  <tr key={a.id} className="cursor-pointer text-[13px] hover:bg-panel-2" onClick={() => open(a.id)}>
                    <Td>
                      <Status value={effectiveStatus(a, now)} />
                    </Td>
                    <Td className="max-w-[180px] truncate text-muted" title={a.requester}>
                      {a.requester}
                    </Td>
                    <Td className="whitespace-nowrap">
                      <button className="font-medium hover:text-accent hover:underline" onClick={(e) => {
                          e.stopPropagation();
                          open(a.id);
                        }}>
                        {actionTitle(a.action)}
                      </button>
                    </Td>
                    <Td className="max-w-[360px] truncate font-mono text-[11px] text-muted" title={`${a.request.method} ${a.request.uri}`}>
                      {a.request.uri ? `${a.request.method} ${a.request.uri}` : "—"}
                    </Td>
                    <Td className="whitespace-nowrap text-xs text-muted" title={time(a.decided_at ?? a.expires_at)}>
                      {a.decided_at ? ago(a.decided_at) : effectiveStatus(a, now) === "expired" ? `expired ${ago(a.expires_at)}` : "—"}
                    </Td>
                  </tr>
                ))}
              </Table>
            )}
          </Panel>
        </div>
      )}

      {id && <ApprovalDetail id={id} fallback={all.find((a) => a.id === id)} onClose={() => open(undefined)} />}
    </div>
  );
}

function ApprovalDetail({ id, fallback, onClose }: { id: string; fallback?: Approval; onClose: () => void }) {
  const q = useApproval(id);
  const me = useMe().data;
  const decide = useDecideApproval();
  const toast = useToast();
  const now = useNow(1_000);
  const [pressed, setPressed] = useState<"approve" | "deny">();
  const [error, setError] = useState<string>();

  const a = q.data?.approval ?? fallback ?? null;
  const status = a ? effectiveStatus(a, now) : "";
  const title = a ? actionTitle(a.action) : "Approval";
  const notOwner = !!a && !!me?.user?.id && !!a.user_id && a.user_id !== me.user.id;

  const send = (approve: boolean) => {
    if (!a) return;
    setPressed(approve ? "approve" : "deny");
    setError(undefined);
    decide.mutate(
      { id: a.id, approve },
      {
        onSuccess: (r) => {
          if (!approve) toast(`Denied: ${title}. Nothing ran.`);
          else if (r?.status === "failed") toast(`${title} ran and failed`, "error");
          else toast(`${title} ran`);
        },
        onError: (e) => {
          const msg = e instanceof ApiError && e.status === 409 ? "This request was already decided or has expired." : errorText(e);
          setError(msg);
          toast(msg, "error");
        },
      },
    );
  };

  return (
    <Modal open onOpenChange={(o) => !o && onClose()} title={title} wide>
      {!a ? (
        q.isLoading ? (
          <Skeleton rows={5} />
        ) : (
          <Note tone="warn">
            {q.data?.status === "missing" ? "This approval does not exist or is not yours." : q.data?.error || "Could not load this approval."}
          </Note>
        )
      ) : (
        <div className="space-y-3 text-[13px]">
          <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-1.5 border-y border-line py-2.5">
            <dt className="text-muted">Status</dt>
            <dd>
              <Status value={status} />
            </dd>
            <dt className="text-muted">Requester</dt>
            <dd className="min-w-0 truncate" title={a.requester}>
              <Link to="/profile/$section" params={{ section: "tokens" }} className="font-medium hover:text-accent hover:underline">
                {a.requester}
              </Link>{" "}
              <span className="text-muted">(AI connector)</span>
            </dd>
            <dt className="text-muted">Tool</dt>
            <dd className="flex min-w-0 items-center gap-1.5">
              <span className="font-medium">{title}</span>
              <span className="truncate font-mono text-[11px] text-muted select-all" title={a.action}>
                {a.action || "—"}
              </span>
            </dd>
            <dt className="text-muted">Asked</dt>
            <dd title={time(a.created_at)}>{a.created_at ? ago(a.created_at) : "—"}</dd>
            {status === "pending" ? (
              <>
                <dt className="text-muted">Expires in</dt>
                <dd className="font-mono text-[12px] tabular-nums" title={time(a.expires_at)}>
                  {left(a.expires_at, now)}
                </dd>
              </>
            ) : a.decided_at ? (
              <>
                <dt className="text-muted">Decided</dt>
                <dd title={time(a.decided_at)}>{ago(a.decided_at)}</dd>
              </>
            ) : status === "expired" ? (
              <>
                <dt className="text-muted">Expired</dt>
                <dd title={time(a.expires_at)}>{a.expires_at ? ago(a.expires_at) : "—"}</dd>
              </>
            ) : null}
            {a.summary && (
              <>
                <dt className="text-muted">Summary</dt>
                <dd className="min-w-0 break-words">{a.summary}</dd>
              </>
            )}
          </dl>

          <section aria-label="Request">
            <div className="mb-1 flex items-center gap-1.5 text-xs font-medium text-warn">
              <TriangleAlert className="size-3.5 shrink-0" /> Data from an AI agent — check it before you approve
            </div>
            <div className="flex min-w-0 items-center gap-2 rounded-t-md border border-line bg-panel-2 px-2.5 py-1.5">
              <Badge tone="accent">{a.request.method}</Badge>
              <code className="min-w-0 flex-1 truncate font-mono text-[12px] select-all" title={a.request.uri}>
                {a.request.uri || "—"}
              </code>
              {a.request.uri && <CopyIcon text={`${a.request.method} ${a.request.uri}`} label="Copy method and URI" />}
            </div>
            <JsonBlock label="Arguments" value={a.request.arguments} className="rounded-t-none border-t-0" />
            {!isEmptyJson(a.request.body) && (
              <details className="mt-1.5">
                <summary className="cursor-pointer text-xs text-muted hover:text-fg">Request body sent to Forge</summary>
                <JsonBlock label="Body" value={a.request.body} className="mt-1" />
              </details>
            )}
          </section>

          {a.mfa_required && (
            <Note tone="warn" icon={<ShieldAlert className="mt-0.5 size-3.5 shrink-0" />}>
              Needs an MFA-verified session.{status === "pending" && me && !me.mfa ? " Forge asks you to verify it's you when you approve." : ""}
            </Note>
          )}

          {status === "pending" && (
            <div className="text-xs text-muted">
              <div className="mb-0.5 font-medium text-fg">What happens</div>
              <ul className="list-disc space-y-0.5 pl-4">
                <li>
                  <strong>Approve and run</strong> sends this request to Forge once, as you, with your permissions. It is audited under your name and the connector's.
                </li>
                <li>The outcome is recorded; the agent reads it when it checks the request's status.</li>
                <li>
                  <strong>Deny</strong> runs nothing; the agent is told you declined.
                </li>
              </ul>
            </div>
          )}

          {status === "executed" && (
            <div>
              <Note tone="ok" icon={<ShieldCheck className="mt-0.5 size-3.5 shrink-0" />}>
                Ran {a.decided_at ? <span title={time(a.decided_at)}>{ago(a.decided_at)}</span> : ""}.
              </Note>
              <details className="mt-1.5">
                <summary className="cursor-pointer text-xs text-muted hover:text-fg">Result</summary>
                <JsonBlock label="Result" value={a.result} className="mt-1" />
              </details>
            </div>
          )}
          {status === "failed" && (
            <div className="rounded-md bg-bad-soft px-3 py-2 text-bad">
              <div className="font-medium">Ran and failed</div>
              <pre className="mt-0.5 font-mono text-[12px] break-words whitespace-pre-wrap select-text">{a.error || "No error details were recorded."}</pre>
            </div>
          )}
          {status === "denied" && <Note tone="muted">You denied this request. Nothing ran.</Note>}
          {status === "expired" && <Note tone="muted">This request expired before anyone decided. Nothing ran; the agent can ask again.</Note>}
          {status === "approved" && (
            <Note tone="muted" icon={<Loader2 className="mt-0.5 size-3.5 shrink-0 animate-spin" />}>
              Approved; running now.
            </Note>
          )}

          {error && (
            <p role="alert" className="rounded-md bg-bad-soft px-3 py-2 text-bad">
              {error}
            </p>
          )}

          <div className="flex flex-wrap items-center justify-end gap-2 border-t border-line pt-3">
            {status === "pending" ? (
              <>
                {notOwner && <span className="mr-auto text-xs text-muted">Only the person whose connector asked can decide.</span>}
                <Button onClick={() => send(false)} disabled={decide.isPending || notOwner}>
                  {decide.isPending && pressed === "deny" && <Loader2 className="size-4 animate-spin" />}
                  Deny
                </Button>
                <Button variant="primary" onClick={() => send(true)} disabled={decide.isPending || notOwner}>
                  {decide.isPending && pressed === "approve" && <Loader2 className="size-4 animate-spin" />}
                  Approve and run
                </Button>
              </>
            ) : (
              <Button onClick={onClose}>Close</Button>
            )}
          </div>
        </div>
      )}
    </Modal>
  );
}

function JsonBlock({ label, value, className = "" }: { label: string; value: unknown; className?: string }) {
  const text = pretty(value);
  return (
    <div className={`relative rounded-md border border-line bg-panel-2 ${className}`}>
      <pre aria-label={label} className="max-h-72 overflow-auto px-2.5 py-2 pr-9 font-mono text-[12px] leading-relaxed break-words whitespace-pre-wrap select-text">
        {text}
      </pre>
      {text !== "—" && (
        <span className="absolute top-1 right-1">
          <CopyIcon text={text} label={`Copy ${label.toLowerCase()}`} />
        </span>
      )}
    </div>
  );
}

function Note({ tone, icon, children }: { tone: "ok" | "warn" | "muted"; icon?: ReactNode; children: ReactNode }) {
  const cls = tone === "ok" ? "bg-ok-soft text-ok" : tone === "warn" ? "bg-warn-soft text-warn" : "bg-panel-2 text-muted";
  return (
    <p className={`flex items-start gap-1.5 rounded-md px-3 py-2 ${cls}`}>
      {icon}
      <span className="min-w-0">{children}</span>
    </p>
  );
}
