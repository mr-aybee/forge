// Minimal API client. The session is an HttpOnly cookie the browser sends
// automatically; mutating requests echo the readable CSRF cookie in a header.
// No token is ever stored in localStorage.

export class ApiError extends Error {
  constructor(
    public status: number,
    public code: string,
    message: string,
    /** The parsed problem+json body, for endpoints that return structured details (e.g. a 409 with options). */
    public body?: unknown,
  ) {
    super(message);
  }
}

export function csrfToken(): string | undefined {
  return document.cookie
    .split("; ")
    .find((c) => c.startsWith("forge_csrf="))
    ?.slice("forge_csrf=".length);
}

/**
 * Called when the server says an action needs an MFA-verified sign-in.
 * Resolves `true` once the user verified a code (the request is then retried
 * once), `false` if they cancelled. Installed by the StepUp provider.
 */
type StepUpHandler = (reason: string) => Promise<boolean>;
let stepUpHandler: StepUpHandler | undefined;
export function setStepUpHandler(h: StepUpHandler | undefined) {
  stepUpHandler = h;
}

export async function api<T>(method: string, path: string, body?: unknown, retried = false): Promise<T> {
  const headers: Record<string, string> = { Accept: "application/json" };
  if (body !== undefined) headers["Content-Type"] = "application/json";
  if (method !== "GET") {
    const csrf = csrfToken();
    if (csrf) headers["X-CSRF-Token"] = csrf;
  }
  const res = await fetch(path, {
    method,
    headers,
    credentials: "same-origin",
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (res.status === 204) return undefined as T;
  const data = await res.json().catch(() => ({}));
  if (res.status === 403 && data.title === "step_up_required" && !retried && stepUpHandler) {
    if (await stepUpHandler(data.detail ?? "This action needs an MFA-verified sign-in.")) return api<T>(method, path, body, true);
  }
  if (!res.ok) {
    throw new ApiError(res.status, data.title ?? "error", data.detail ?? res.statusText, data);
  }
  return data as T;
}

export const get = <T>(path: string) => api<T>("GET", path);
export const post = <T>(path: string, body: unknown = {}) => api<T>("POST", path, body);
