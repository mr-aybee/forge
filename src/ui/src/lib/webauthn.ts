// Passkeys (WebAuthn) for the console. The server speaks base64url JSON; the
// browser API wants ArrayBuffers. Nothing here is stored: challenges and
// signatures go straight from the server to the authenticator and back.

import { useQuery } from "@tanstack/react-query";

import { ApiError, get } from "../api/client";

// ------------------------------------------------------------------ base64url

export function toB64url(buf: ArrayBuffer | ArrayBufferView): string {
  const bytes = buf instanceof ArrayBuffer ? new Uint8Array(buf) : new Uint8Array(buf.buffer, buf.byteOffset, buf.byteLength);
  let s = "";
  for (let i = 0; i < bytes.length; i++) s += String.fromCharCode(bytes[i] ?? 0);
  return btoa(s).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

export function fromB64url(s: string): ArrayBuffer {
  const b64 = s.replace(/-/g, "+").replace(/_/g, "/");
  const bin = atob(b64 + "=".repeat((4 - (b64.length % 4)) % 4));
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out.buffer;
}

// ------------------------------------------------------------------ support

export type PasskeySupport = { ok: true } | { ok: false; reason: string };

/** Whether this page can use passkeys. Browsers only allow WebAuthn on HTTPS or localhost. */
export function passkeySupport(): PasskeySupport {
  if (typeof window === "undefined") return { ok: false, reason: "Passkeys are not available here." };
  if (!window.isSecureContext) return { ok: false, reason: "Passkeys need HTTPS (or localhost)." };
  if (!window.PublicKeyCredential || !navigator.credentials) return { ok: false, reason: "This browser does not support passkeys." };
  return { ok: true };
}

/** True when the user dismissed the browser prompt or it timed out: not worth an error toast. */
export const isPasskeyCancel = (e: unknown) => e instanceof DOMException && (e.name === "NotAllowedError" || e.name === "AbortError");

/** A readable message for anything thrown by the passkey flows. */
export function passkeyErrorText(e: unknown): string {
  if (e instanceof DOMException) {
    if (e.name === "NotAllowedError" || e.name === "AbortError") return "The passkey prompt was cancelled or timed out.";
    if (e.name === "InvalidStateError") return "This authenticator already holds a passkey for your account.";
    if (e.name === "SecurityError") return "The browser refused: the console address does not match the passkey domain.";
    if (e.name === "NotSupportedError") return "This authenticator does not support the required key types.";
  }
  return e instanceof Error ? e.message : String(e);
}

// ------------------------------------------------------------------ ceremonies

type Json = Record<string, unknown>;
interface CredRef {
  id?: unknown;
  type?: unknown;
  transports?: unknown;
}

const obj = (v: unknown): Json => (v && typeof v === "object" ? (v as Json) : {});
const str = (v: unknown) => (typeof v === "string" ? v : undefined);

function need(v: unknown, what: string): string {
  const s = str(v);
  if (!s) throw new Error(`The server sent incomplete passkey options (${what}).`);
  return s;
}

function credList(v: unknown): PublicKeyCredentialDescriptor[] {
  return (Array.isArray(v) ? (v as CredRef[]) : [])
    .filter((c) => typeof c?.id === "string" && c.id)
    .map((c) => ({
      type: "public-key" as const,
      id: fromB64url(c.id as string),
      ...(Array.isArray(c.transports) ? { transports: c.transports as AuthenticatorTransport[] } : {}),
    }));
}

const timeout = (v: unknown) => (typeof v === "number" && v > 0 ? v : 120_000);

/** Body of `POST /me/passkeys/register/finish`. */
export interface RegistrationBody {
  client_data_json: string;
  attestation_object: string;
  name: string;
}

/** Body of `POST /auth/passkey/finish` and `/auth/step-up/passkey/finish`. */
export interface AssertionBody {
  credential_id: string;
  client_data_json: string;
  authenticator_data: string;
  signature: string;
}

/** Runs `navigator.credentials.create` with the server's creation options. */
export async function createPasskey(options: unknown, name: string): Promise<RegistrationBody> {
  const support = passkeySupport();
  if (!support.ok) throw new Error(support.reason);
  const o = obj(options);
  const rp = obj(o.rp);
  const user = obj(o.user);
  const params = (Array.isArray(o.pubKeyCredParams) ? o.pubKeyCredParams : [])
    .map(obj)
    .filter((p) => typeof p.alg === "number")
    .map((p) => ({ type: "public-key" as const, alg: p.alg as number }));
  const publicKey: PublicKeyCredentialCreationOptions = {
    challenge: fromB64url(need(o.challenge, "challenge")),
    rp: { name: str(rp.name) ?? "Orbyn Forge", ...(str(rp.id) ? { id: str(rp.id) } : {}) },
    user: {
      id: fromB64url(need(user.id, "user id")),
      name: str(user.name) ?? "user",
      displayName: str(user.displayName) ?? str(user.name) ?? "user",
    },
    pubKeyCredParams: params.length ? params : [{ type: "public-key", alg: -7 }, { type: "public-key", alg: -257 }],
    timeout: timeout(o.timeout),
    attestation: (str(o.attestation) as AttestationConveyancePreference | undefined) ?? "none",
    authenticatorSelection: obj(o.authenticatorSelection) as AuthenticatorSelectionCriteria,
    excludeCredentials: credList(o.excludeCredentials),
  };
  const cred = (await navigator.credentials.create({ publicKey })) as PublicKeyCredential | null;
  if (!cred) throw new Error("No passkey was created.");
  const r = cred.response as AuthenticatorAttestationResponse;
  return { client_data_json: toB64url(r.clientDataJSON), attestation_object: toB64url(r.attestationObject), name };
}

/** Runs `navigator.credentials.get` with the server's request options. */
export async function getPasskey(options: unknown): Promise<AssertionBody> {
  const support = passkeySupport();
  if (!support.ok) throw new Error(support.reason);
  const o = obj(options);
  const publicKey: PublicKeyCredentialRequestOptions = {
    challenge: fromB64url(need(o.challenge, "challenge")),
    timeout: timeout(o.timeout),
    userVerification: (str(o.userVerification) as UserVerificationRequirement | undefined) ?? "required",
    allowCredentials: credList(o.allowCredentials),
    ...(str(o.rpId) ? { rpId: str(o.rpId) } : {}),
  };
  const cred = (await navigator.credentials.get({ publicKey })) as PublicKeyCredential | null;
  if (!cred) throw new Error("No passkey was selected.");
  const r = cred.response as AuthenticatorAssertionResponse;
  return {
    credential_id: toB64url(cred.rawId),
    client_data_json: toB64url(r.clientDataJSON),
    authenticator_data: toB64url(r.authenticatorData),
    signature: toB64url(r.signature),
  };
}

// ------------------------------------------------------------------ your passkeys

export interface PasskeyInfo {
  id: string;
  name: string;
  created_at?: number | null;
  last_used_at?: number | null;
}

export interface PasskeyList {
  /** False when the server has no passkey endpoints (older release). */
  available: boolean;
  items: PasskeyInfo[];
  /** A failed request, other than "not available". */
  error?: string;
}

export const PASSKEYS_KEY = ["me", "passkeys"] as const;

const num = (v: unknown) => (typeof v === "number" && Number.isFinite(v) ? v : null);

/** Your passkeys; always a complete list, never throws. */
export const usePasskeys = (enabled = true) =>
  useQuery({
    queryKey: PASSKEYS_KEY,
    enabled,
    staleTime: 30_000,
    queryFn: async (): Promise<PasskeyList> => {
      try {
        const r = await get<unknown>("/api/v1/me/passkeys");
        const items = (Array.isArray(r) ? r : [])
          .map(obj)
          .filter((p) => typeof p.id === "string" && p.id)
          .map((p) => ({ id: p.id as string, name: str(p.name) || "Passkey", created_at: num(p.created_at), last_used_at: num(p.last_used_at) }));
        return { available: true, items };
      } catch (e) {
        if (e instanceof ApiError && (e.status === 404 || e.status === 405)) return { available: false, items: [] };
        return { available: true, items: [], error: e instanceof Error ? e.message : String(e) };
      }
    },
  });
