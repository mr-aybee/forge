import { useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { Fingerprint, Loader2, ShieldCheck } from "lucide-react";
import { createContext, useCallback, useContext, useEffect, useRef, useState, type ReactNode } from "react";

import { ApiError, api, setStepUpHandler } from "../api/client";
import { useMe } from "../api/hooks";
import { Button, Input, Modal } from "../components/ui";
import { getPasskey, isPasskeyCancel, passkeyErrorText, passkeySupport, usePasskeys } from "../lib/webauthn";

interface Ctx {
  /** Ensures the session is MFA-verified, asking for a code if needed. */
  ensureMfa(reason?: string): Promise<boolean>;
}

const StepUpContext = createContext<Ctx>({ ensureMfa: async () => true });

/**
 * Step-up MFA: sensitive actions (packages, terminals, proxies…) need an
 * MFA-verified sign-in. Instead of failing, the console asks for an
 * authenticator code, upgrades the session and retries.
 */
export function StepUpProvider({ children }: { children: ReactNode }) {
  const me = useMe().data;
  const qc = useQueryClient();
  const [ask, setAsk] = useState<{ reason: string } | null>(null);
  const [code, setCode] = useState("");
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
  const pending = useRef<((ok: boolean) => void)[]>([]);

  const prompt = useCallback((reason: string) => {
    setCode("");
    setError(undefined);
    setAsk({ reason });
    return new Promise<boolean>((resolve) => pending.current.push(resolve));
  }, []);

  const finish = (ok: boolean) => {
    for (const r of pending.current.splice(0)) r(ok);
    setAsk(null);
  };

  useEffect(() => {
    setStepUpHandler(prompt);
    return () => setStepUpHandler(undefined);
  }, [prompt]);

  const ensureMfa = useCallback(
    async (reason = "This needs an MFA-verified sign-in.") => (me?.mfa ? true : prompt(reason)),
    [me?.mfa, prompt],
  );

  const enrolled = me?.mfa_enrolled ?? false;
  // Only asked while the dialog is open; an older server simply has none.
  const keys = usePasskeys(!!ask && me?.session_kind !== "bearer");
  const hasPasskey = (keys.data?.items.length ?? 0) > 0;
  const support = passkeySupport();
  const preferKey = hasPasskey && support.ok;
  const [keyBusy, setKeyBusy] = useState(false);

  // Same outcome as a code: the server swaps in an MFA session (new CSRF
  // cookie), `me` is refreshed and the waiting request is retried.
  const withPasskey = async () => {
    setKeyBusy(true);
    setError(undefined);
    try {
      const options = await api<unknown>("POST", "/api/v1/auth/step-up/passkey/begin", {});
      const assertion = await getPasskey(options);
      await api("POST", "/api/v1/auth/step-up/passkey/finish", assertion);
      await qc.invalidateQueries({ queryKey: ["me"] });
      finish(true);
    } catch (err) {
      if (!isPasskeyCancel(err)) setError(passkeyErrorText(err));
    } finally {
      setKeyBusy(false);
    }
  };

  const passkeyButton = hasPasskey && (
    <div>
      <Button
        type="button"
        variant={preferKey ? "primary" : "default"}
        className="w-full py-2"
        autoFocus={preferKey}
        disabled={!support.ok || keyBusy || busy}
        title={support.ok ? undefined : support.reason}
        onClick={() => void withPasskey()}
      >
        {keyBusy ? <Loader2 className="size-4 animate-spin" /> : <Fingerprint className="size-4" />}
        {keyBusy ? "Waiting for your passkey…" : "Use a passkey"}
      </Button>
      {!support.ok && <p className="mt-1.5 text-xs text-muted">{support.reason}</p>}
    </div>
  );

  const divider = (
    <div className="flex items-center gap-3 text-[11px] tracking-wide text-faint uppercase" aria-hidden>
      <span className="h-px flex-1 bg-line" />
      or enter a code
      <span className="h-px flex-1 bg-line" />
    </div>
  );

  return (
    <StepUpContext.Provider value={{ ensureMfa }}>
      {children}
      <Modal
        open={!!ask}
        onOpenChange={(o) => !o && finish(false)}
        title="Verify it's you"
        description={ask?.reason}
      >
        {enrolled ? (
          <form
            className="space-y-4"
            onSubmit={async (e) => {
              e.preventDefault();
              setBusy(true);
              setError(undefined);
              try {
                await api("POST", "/api/v1/auth/step-up", { code: code.replace(/\s/g, "") });
                await qc.invalidateQueries({ queryKey: ["me"] });
                finish(true);
              } catch (err) {
                setError(err instanceof ApiError ? err.message : "Verification failed");
              } finally {
                setBusy(false);
              }
            }}
          >
            <div className="flex items-start gap-3 rounded-lg bg-accent-soft p-3 text-[13px]">
              <ShieldCheck className="mt-0.5 size-5 shrink-0 text-accent" />
              <span>
                {hasPasskey ? "Use your passkey, or enter the 6-digit code from your authenticator app." : "Enter the 6-digit code from your authenticator app."} Your sign-in
                stays verified until it expires.
              </span>
            </div>
            {passkeyButton}
            {hasPasskey && divider}
            <Input
              autoFocus={!preferKey}
              aria-label="6-digit code"
              inputMode="numeric"
              autoComplete="one-time-code"
              pattern="[0-9 ]{6,7}"
              placeholder="123 456"
              className="text-center font-mono text-lg tracking-[0.3em]"
              value={code}
              onChange={(e) => setCode(e.target.value)}
              required
            />
            {error && <p className="text-[13px] text-bad">{error}</p>}
            <div className="flex justify-end gap-2">
              <Button type="button" variant="ghost" onClick={() => finish(false)}>
                Cancel
              </Button>
              <Button variant={preferKey ? "default" : "primary"} disabled={busy || keyBusy}>
                {busy && <Loader2 className="size-4 animate-spin" />}
                Verify and continue
              </Button>
            </div>
          </form>
        ) : hasPasskey ? (
          <div className="space-y-4 text-[13px]">
            <div className="flex items-start gap-3 rounded-lg bg-accent-soft p-3">
              <ShieldCheck className="mt-0.5 size-5 shrink-0 text-accent" />
              <span>Confirm with your passkey. Your sign-in stays verified until it expires.</span>
            </div>
            {passkeyButton}
            {error && <p className="text-bad">{error}</p>}
            <div className="flex justify-end">
              <Button type="button" variant="ghost" onClick={() => finish(false)}>
                Cancel
              </Button>
            </div>
          </div>
        ) : keys.isLoading ? (
          <p className="flex items-center gap-2 text-[13px] text-muted">
            <Loader2 className="size-4 animate-spin" /> Checking your sign-in methods…
          </p>
        ) : (
          <div className="space-y-4 text-[13px]">
            <p>
              Your account has no authenticator app or passkey yet. Set one up under <strong>Profile → Password &amp; security</strong>, then try again — you won't need to sign
              out.
            </p>
            <div className="flex justify-end gap-2">
              <Button variant="ghost" onClick={() => finish(false)}>
                Cancel
              </Button>
              <Link to="/profile/$section" params={{ section: "security" }} onClick={() => finish(false)}>
                <Button variant="primary">Open settings</Button>
              </Link>
            </div>
          </div>
        )}
      </Modal>
    </StepUpContext.Provider>
  );
}

export const useStepUp = () => useContext(StepUpContext);
