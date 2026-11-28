import { useQueryClient } from "@tanstack/react-query";
import { Fingerprint, Hexagon, KeyRound, Loader2, RefreshCw, ShieldCheck } from "lucide-react";
import { useState, type FormEvent } from "react";

import { ApiError, get, post } from "../api/client";
import { Button, Field, Input } from "../components/ui";
import { solveLoginChallenge } from "../lib/pow";
import { getPasskey, isPasskeyCancel, passkeyErrorText, passkeySupport } from "../lib/webauthn";

export function Login({ ssoEnabled }: { ssoEnabled: boolean }) {
  const qc = useQueryClient();
  const [login, setLogin] = useState("");
  const [password, setPassword] = useState("");
  const [totp, setTotp] = useState("");
  const [needTotp, setNeedTotp] = useState(false);
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
  const [keyBusy, setKeyBusy] = useState(false);
  const [checking, setChecking] = useState(false);
  // Image CAPTCHA after failed attempts (server-drawn PNG, single use).
  const [captcha, setCaptcha] = useState<{ id: string; image: string; length: number }>();
  const [captchaAnswer, setCaptchaAnswer] = useState("");
  const [captchaBusy, setCaptchaBusy] = useState(false);
  const support = passkeySupport();

  // Asks whether the next attempt needs a picture and fetches a new one.
  const loadCaptcha = async () => {
    setCaptchaBusy(true);
    try {
      const r = await get<{ required: boolean; id?: string; image?: string; length?: number }>(
        `/api/v1/auth/captcha?login=${encodeURIComponent(login)}`,
      );
      setCaptcha(r.required && r.id && r.image ? { id: r.id, image: r.image, length: r.length ?? 6 } : undefined);
    } catch {
      setCaptcha(undefined);
    } finally {
      setCaptchaAnswer("");
      setCaptchaBusy(false);
    }
  };

  // Same handling for every sign-in method: the server sets the session and
  // CSRF cookies; follow a same-site OAuth `next`, else load the console.
  const signedIn = async () => {
    const next = new URLSearchParams(location.search).get("next");
    // Only same-site OAuth authorize paths are followed (no open redirect).
    if (next?.startsWith("/oauth/")) location.assign(next);
    else await qc.invalidateQueries({ queryKey: ["me"] });
  };

  const withPasskey = async () => {
    setKeyBusy(true);
    setError(undefined);
    try {
      const options = await post<unknown>("/api/v1/auth/passkey/begin");
      const assertion = await getPasskey(options);
      await post("/api/v1/auth/passkey/finish", assertion);
      await signedIn();
    } catch (err) {
      if (!isPasskeyCancel(err)) {
        setError(err instanceof ApiError && err.status === 404 ? "This server does not support passkeys yet." : passkeyErrorText(err));
      }
    } finally {
      setKeyBusy(false);
    }
  };

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setChecking(false);
    setError(undefined);
    try {
      // Every attempt needs a fresh, solved challenge (single use).
      const attempt = async () => {
        const answer = await solveLoginChallenge(login, () => setChecking(true));
        setChecking(false);
        await post("/api/v1/auth/login", {
          login,
          password,
          totp: needTotp ? totp : undefined,
          ...answer,
          ...(captcha ? { captcha_id: captcha.id, captcha_answer: captchaAnswer } : {}),
        });
      };
      try {
        await attempt();
      } catch (err) {
        // A challenge that expired or became harder meanwhile: once more.
        if (err instanceof ApiError && (err.code === "challenge_failed" || err.code === "challenge_required")) await attempt();
        else throw err;
      }
      await signedIn();
    } catch (err) {
      if (err instanceof ApiError && err.code === "mfa_required") setNeedTotp(true);
      else {
        setError(
          err instanceof ApiError && err.code === "captcha_required"
            ? "Type the characters in the picture to continue."
            : err instanceof Error
              ? err.message
              : "Sign-in failed",
        );
        // Every answer uses the picture up; after a failure the next attempt may need one.
        await loadCaptcha();
      }
    } finally {
      setBusy(false);
      setChecking(false);
    }
  };

  return (
    <main className="grid min-h-screen lg:grid-cols-2">
      <section className="ocean-hero hidden flex-col justify-between p-10 text-white lg:flex">
        <div className="flex items-center gap-2 text-lg font-semibold">
          <Hexagon className="size-7 fill-white/10" /> Orbyn Forge
        </div>
        <div>
          <h2 className="max-w-md text-3xl font-semibold tracking-tight">One secure workspace for every server, cluster and service.</h2>
          <ul className="mt-6 space-y-2 text-sm text-white/80">
            <li className="flex gap-2"><ShieldCheck className="size-4" /> Agents dial out over mTLS — no inbound ports</li>
            <li className="flex gap-2"><ShieldCheck className="size-4" /> Every action authorized by policy and audited</li>
            <li className="flex gap-2"><ShieldCheck className="size-4" /> Windows and Linux, side by side</li>
          </ul>
        </div>
        <p className="text-xs text-white/50">Self-hosted control plane</p>
      </section>
      <section className="grid place-items-center px-4">
        <form onSubmit={submit} className="w-full max-w-sm space-y-4">
          <div>
            <h1 className="text-xl font-semibold">Sign in</h1>
            <p className="text-muted">Use your Orbyn Forge account.</p>
          </div>
          <Field label="Username or email">
            <Input autoComplete="username" autoCapitalize="none" spellCheck={false} required autoFocus value={login} onChange={(e) => setLogin(e.target.value)} />
          </Field>
          <Field label="Password">
            <Input type="password" autoComplete="current-password" required value={password} onChange={(e) => setPassword(e.target.value)} />
          </Field>
          {needTotp && (
            <Field label="One-time code" hint="From your authenticator app">
              <Input inputMode="numeric" autoComplete="one-time-code" pattern="[0-9]{6}" autoFocus value={totp} onChange={(e) => setTotp(e.target.value)} />
            </Field>
          )}
          {captcha && (
            <Field label="Characters in the picture" hint="Letters and digits, not case-sensitive. No audio alternative yet.">
              <div className="flex items-center gap-2">
                <img src={captcha.image} alt="Picture of characters to type (security check)" width={220} height={80} className="rounded-md border border-line bg-white" />
                <Button type="button" title="New picture" aria-label="New picture" disabled={captchaBusy} onClick={() => void loadCaptcha()}>
                  <RefreshCw className={`size-4 ${captchaBusy ? "animate-spin" : ""}`} />
                </Button>
              </div>
              <Input
                className="mt-2"
                required
                autoComplete="off"
                autoCapitalize="characters"
                spellCheck={false}
                maxLength={captcha.length + 4}
                value={captchaAnswer}
                onChange={(e) => setCaptchaAnswer(e.target.value)}
              />
            </Field>
          )}
          {error && (
            <p role="alert" className="rounded-md bg-bad-soft px-3 py-2 text-bad">
              {error}
            </p>
          )}
          <Button type="submit" variant="primary" disabled={busy || keyBusy} className="w-full py-2">
            {busy ? (checking ? "Checking your browser…" : "Signing in…") : "Sign in"}
          </Button>
          <div className="flex items-center gap-3 text-[11px] tracking-wide text-faint uppercase" aria-hidden>
            <span className="h-px flex-1 bg-line" />
            or
            <span className="h-px flex-1 bg-line" />
          </div>
          <div>
            <Button
              type="button"
              className="w-full py-2"
              disabled={!support.ok || busy || keyBusy}
              title={support.ok ? undefined : support.reason}
              onClick={() => void withPasskey()}
            >
              {keyBusy ? <Loader2 className="size-4 animate-spin" /> : <Fingerprint className="size-4" />}
              {keyBusy ? "Waiting for your passkey…" : "Sign in with a passkey"}
            </Button>
            {!support.ok && <p className="mt-1.5 text-center text-xs text-muted">{support.reason}</p>}
          </div>
          {ssoEnabled && (
            <a href="/api/v1/auth/oidc/login" className="flex items-center justify-center gap-2 rounded-md border border-line py-2 hover:border-accent/60">
              <KeyRound className="size-4" /> Sign in with SSO
            </a>
          )}
        </form>
      </section>
    </main>
  );
}
