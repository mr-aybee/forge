import { useSearch } from "@tanstack/react-router";
import { CheckCircle2, Hexagon, Loader2, Terminal, TriangleAlert, XCircle } from "lucide-react";
import { useState, type FormEvent } from "react";

import { ApiError, api } from "../api/client";
import { Button, Field, Input } from "../components/ui";

type DeviceRequest = {
  user_code: string;
  client_name: string;
  client_id: string;
  requested_seconds_ago: number;
  expires_in: number;
  user: string;
  mfa: boolean;
  requested_from: string;
  user_agent: string;
};

const NOT_FOUND = "No pending sign-in with that code. Check it, or run the sign-in on the device again.";

/**
 * Approves a device sign-in (`forge auth login --device`, RFC 8628). The
 * person types the code the device shows; nothing is issued until they
 * approve here. Rendered without the console shell.
 */
export function Device() {
  const { code: initial } = useSearch({ from: "/device" });
  const [code, setCode] = useState(initial);
  const [req, setReq] = useState<DeviceRequest>();
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState<"approved" | "denied">();

  const look = async (e?: FormEvent) => {
    e?.preventDefault();
    setBusy(true);
    setError(undefined);
    try {
      setReq(await api<DeviceRequest>("GET", `/api/v1/oauth/device/${encodeURIComponent(code.trim())}`));
    } catch (err) {
      setReq(undefined);
      setError(err instanceof ApiError && err.status === 404 ? NOT_FOUND : err instanceof Error ? err.message : "Could not check the code.");
    } finally {
      setBusy(false);
    }
  };

  const decide = async (approve: boolean) => {
    if (!req) return;
    setBusy(true);
    setError(undefined);
    try {
      await api("POST", `/api/v1/oauth/device/${encodeURIComponent(req.user_code)}`, { approve });
      setDone(approve ? "approved" : "denied");
    } catch (err) {
      setError(err instanceof ApiError && err.status === 404 ? NOT_FOUND : err instanceof Error ? err.message : "Could not answer.");
    } finally {
      setBusy(false);
    }
  };

  return (
    <main className="grid min-h-screen place-items-center bg-bg px-4 py-8">
      <div className="w-full max-w-md">
        <div className="mb-3 flex items-center gap-2 text-[13px] font-semibold text-muted">
          <Hexagon className="size-4 text-accent" /> Orbyn Forge
        </div>
        <section className="space-y-4 rounded-lg border border-line bg-panel p-5 shadow-[var(--shadow-card)]">
          <div className="flex items-center gap-2">
            <Terminal className="size-4 text-accent" />
            <h1 className="text-base font-semibold">Sign in a device</h1>
          </div>
          {done ? (
            <p role="status" className="flex items-start gap-2 text-[13px]">
              {done === "approved" ? <CheckCircle2 className="mt-0.5 size-4 text-ok" /> : <XCircle className="mt-0.5 size-4 text-bad" />}
              {done === "approved" ? "Approved. The device signs in within a few seconds; you can close this page." : "Denied. The device was not signed in."}
            </p>
          ) : !req ? (
            <form onSubmit={(e) => void look(e)} className="space-y-3">
              <p className="text-[13px] text-muted">Enter the code shown by <code>forge auth login --device</code>.</p>
              <Field label="Code">
                <Input
                  autoFocus
                  autoCapitalize="characters"
                  autoComplete="off"
                  spellCheck={false}
                  placeholder="BCDF-GHJK"
                  className="font-mono tracking-widest uppercase"
                  value={code}
                  onChange={(e) => setCode(e.target.value)}
                />
              </Field>
              <Button type="submit" variant="primary" className="w-full" disabled={busy || code.trim().length < 8}>
                {busy ? <Loader2 className="size-4 animate-spin" /> : null} Continue
              </Button>
            </form>
          ) : (
            <div className="space-y-3 text-[13px]">
              <p>
                <b>{req.client_name}</b> wants to sign in as <b>{req.user}</b>, from <b>{req.requested_from}</b> ({req.user_agent}).
                Approve only if you started this sign-in yourself on that machine.
              </p>
              <p className="rounded-md border border-line bg-bg px-3 py-2 text-center font-mono text-lg tracking-widest">{req.user_code}</p>
              <p className="flex items-start gap-2 text-muted">
                <TriangleAlert className="mt-0.5 size-4 shrink-0 text-warn" />
                Approve only if this code is on the screen of a device you are using right now. Someone who sent you this link can be trying to sign in as you.
              </p>
              <p className="text-muted">The device can run MFA-protected actions only after it asks for a code.</p>
              <div className="flex gap-2">
                <Button variant="primary" className="flex-1" disabled={busy} onClick={() => void decide(true)}>
                  Approve
                </Button>
                <Button className="flex-1" disabled={busy} onClick={() => void decide(false)}>
                  Deny
                </Button>
              </div>
            </div>
          )}
          {error && (
            <p role="alert" className="rounded-md bg-bad-soft px-3 py-2 text-[13px] text-bad">
              {error}
            </p>
          )}
        </section>
      </div>
    </main>
  );
}
