import { useSearch } from "@tanstack/react-router";
import { ArrowRight, Eye, Hexagon, Loader2, ShieldCheck, TriangleAlert, UserRound, Zap } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";

import { ApiError } from "../api/client";
import { useAnswerConsent, useConsentRequest, type AgentMode, type ConsentRequest } from "../api/connectors";
import { errorText } from "../app/toast";
import { Button, CopyIcon, Skeleton, ago, time } from "../components/ui";

const EXPIRED = "This sign-in request expired or was already answered. Start again from the app.";

const MODE_INFO: Record<AgentMode, { label: string; icon: typeof Eye; text: string }> = {
  read_only: { label: "Read-only", icon: Eye, text: "Only looks: lists, reads and logs." },
  ask: { label: "Ask", icon: UserRound, text: "Looks freely; every change waits for your approval in Forge." },
  auto: { label: "Auto", icon: Zap, text: "Also runs reversible changes, such as restarting stack services or cancelling jobs." },
};

/**
 * Consent for an MCP connector (issue #6). The server sends the browser here
 * from `/oauth/authorize` once the person is signed in; nothing is issued
 * until they approve. Rendered without the console shell.
 */
export function Connect() {
  const { request: id } = useSearch({ from: "/connect" });
  const q = useConsentRequest(id);
  const r = q.data;

  return (
    <main className="grid min-h-screen place-items-center bg-bg px-4 py-8">
      <div className="w-full max-w-lg">
        <div className="mb-3 flex items-center gap-2 text-[13px] font-semibold text-muted">
          <Hexagon className="size-4 text-accent" /> Orbyn Forge
        </div>
        <section className="rounded-lg border border-line bg-panel shadow-[var(--shadow-card)]">
          {!id ? (
            <Problem>This page is opened by an app that connects to Forge. Start again from the app.</Problem>
          ) : q.isLoading ? (
            <Skeleton rows={5} />
          ) : !r || r.status === "missing" ? (
            <Problem>{EXPIRED}</Problem>
          ) : r.status !== "ok" || !r.request ? (
            <Problem>{r.error || "Could not load this sign-in request."}</Problem>
          ) : (
            <ConsentForm req={r.request} />
          )}
        </section>
      </div>
    </main>
  );
}

function Problem({ children }: { children: ReactNode }) {
  return (
    <div role="alert" className="flex items-start gap-2 p-5 text-[13px]">
      <TriangleAlert className="mt-0.5 size-4 shrink-0 text-warn" />
      <p>{children}</p>
    </div>
  );
}

function ConsentForm({ req }: { req: ConsentRequest }) {
  const answer = useAnswerConsent(req.request_id);
  const initial = req.default_mode && req.modes.includes(req.default_mode) ? req.default_mode : req.modes[0];
  const [mode, setMode] = useState<AgentMode | undefined>(initial);
  const [pressed, setPressed] = useState<"approve" | "deny">();
  const [leaving, setLeaving] = useState(false);
  const [error, setError] = useState<string>();

  // Keep the choice valid if the request is reloaded with fewer modes.
  useEffect(() => {
    if (mode && !req.modes.includes(mode)) setMode(initial);
  }, [req.modes, mode, initial]);

  const busy = answer.isPending || leaving;
  const send = (approve: boolean) => {
    if (approve && !mode) return;
    setPressed(approve ? "approve" : "deny");
    setError(undefined);
    answer.mutate(approve && mode ? { approve: true, mode } : { approve: false }, {
      onSuccess: (redirect) => {
        setLeaving(true);
        window.location.href = redirect;
      },
      onError: (e) => setError(e instanceof ApiError && e.status === 404 ? EXPIRED : errorText(e)),
    });
  };

  return (
    <form
      onSubmit={(e) => {
        e.preventDefault();
        send(true);
      }}
    >
      <header className="border-b border-line px-5 py-3">
        <h1 className="text-base font-semibold">Connect an app to Forge</h1>
        <p className="text-xs text-muted">An app asks to act for you through Forge's MCP endpoint.</p>
      </header>

      <dl className="divide-y divide-line text-[13px]">
        <div className="px-5 py-3">
          <dt className="text-[11px] font-medium tracking-wide text-faint uppercase">Name given by the app</dt>
          <dd className="mt-0.5 min-w-0">
            <div className="truncate text-[15px] font-semibold" title={req.client.name}>
              {req.client.name}
            </div>
            <div className="flex min-w-0 items-center gap-1 text-[11px] text-faint">
              <span className="min-w-0 truncate font-mono tabular-nums select-all" title={req.client.client_id}>
                {req.client.client_id || "—"}
              </span>
              {req.client.registered_at && (
                <span className="shrink-0" title={time(req.client.registered_at)}>
                  · registered {ago(req.client.registered_at)}
                </span>
              )}
            </div>
            <p className="mt-1 text-xs text-muted">Any app can choose this name. Check where you go next before you approve.</p>
          </dd>
        </div>

        <div className="px-5 py-3">
          <dt className="text-[11px] font-medium tracking-wide text-faint uppercase">Where you go next</dt>
          <dd className="mt-0.5 min-w-0">
            <div className="flex min-w-0 items-center gap-1.5">
              <ArrowRight className="size-3.5 shrink-0 text-muted" />
              <span className="shrink-0 text-muted">After you approve, you go to</span>
              <strong className="min-w-0 truncate font-mono text-[13px] text-fg" title={req.redirect_host}>
                {req.redirect_host || "—"}
              </strong>
            </div>
            <div className="mt-1 flex items-start gap-1">
              <code className="min-w-0 flex-1 rounded border border-line bg-panel-2 px-2 py-1 font-mono text-[11px] break-all select-all">{req.redirect_uri || "—"}</code>
              {req.redirect_uri && <CopyIcon text={req.redirect_uri} label="Copy redirect URI" />}
            </div>
          </dd>
        </div>

        <div className="flex min-w-0 items-center gap-1.5 px-5 py-3">
          <dt className="flex shrink-0 items-center gap-1.5 text-muted">
            <UserRound className="size-3.5" /> Connect as
          </dt>
          <dd className="min-w-0 truncate font-medium" title={req.user}>
            {req.user || "—"}
          </dd>
        </div>
      </dl>

      <fieldset className="border-t border-line px-5 py-3" disabled={busy}>
        <legend className="sr-only">What the app may do</legend>
        <div className="mb-1.5 text-[11px] font-medium tracking-wide text-faint uppercase">What it may do</div>
        {req.modes.length === 0 ? (
          <p className="text-xs text-warn">This Forge allows no connector access. Ask an administrator.</p>
        ) : (
          <div className="space-y-1">
            {req.modes.map((m) => {
              const info = MODE_INFO[m];
              const Icon = info.icon;
              return (
                <label
                  key={m}
                  className={`flex cursor-pointer items-start gap-2 rounded-md border px-3 py-2 transition has-[:focus-visible]:ring-2 has-[:focus-visible]:ring-accent/50 ${
                    mode === m ? "border-accent/60 bg-accent-soft" : "border-line hover:border-line-strong"
                  }`}
                >
                  <input type="radio" name="mode" value={m} checked={mode === m} onChange={() => setMode(m)} className="mt-0.5 accent-accent" />
                  <span className="min-w-0">
                    <span className="flex items-center gap-1.5 text-[13px] font-medium">
                      <Icon className="size-3.5" /> {info.label}
                      {m === req.default_mode && <span className="text-[11px] font-normal text-faint">default</span>}
                    </span>
                    <span className="block text-xs text-muted">{info.text}</span>
                  </span>
                </label>
              );
            })}
          </div>
        )}
        <p className="mt-2 flex items-start gap-1.5 text-xs text-muted">
          <ShieldCheck className="mt-0.5 size-3.5 shrink-0 text-ok" />
          Destructive and multi-factor-protected actions always need you in the console. Every call is audited and runs with your permissions.
        </p>
        {req.resource && (
          <p className="mt-1 truncate text-[11px] text-faint" title={req.resource}>
            Access to <span className="font-mono select-all">{req.resource}</span> only
          </p>
        )}
      </fieldset>

      {error && (
        <p role="alert" className="mx-5 mb-3 rounded-md bg-bad-soft px-3 py-2 text-[13px] text-bad">
          {error}
        </p>
      )}

      <div className="flex flex-wrap items-center justify-end gap-2 border-t border-line px-5 py-3">
        {leaving && <span className="mr-auto text-xs text-muted">Going back to the app…</span>}
        <Button type="button" onClick={() => send(false)} disabled={busy}>
          {busy && pressed === "deny" && <Loader2 className="size-4 animate-spin" />}
          Deny
        </Button>
        <Button type="submit" variant="primary" disabled={busy || !mode} title={!mode ? "This Forge allows no connector access" : undefined}>
          {busy && pressed === "approve" && <Loader2 className="size-4 animate-spin" />}
          Approve
        </Button>
      </div>
    </form>
  );
}
