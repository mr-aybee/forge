// Login challenge (D-064): fetch a proof-of-work challenge for the sign-in
// name and solve it in a Web Worker, so the page stays responsive.

type Challenge = { required: boolean; id?: string; salt?: string; difficulty?: number };
export type ChallengeAnswer = { challenge_id?: string; nonce?: string };

const MAX_DIFFICULTY = 26;

/**
 * Returns the fields to add to the login body. `onSlow` runs when solving
 * takes more than a second (show "Checking your browser…").
 */
export async function solveLoginChallenge(login: string, onSlow?: () => void): Promise<ChallengeAnswer> {
  const r = await fetch(`/api/v1/auth/challenge?login=${encodeURIComponent(login.trim())}`, { credentials: "same-origin" });
  if (!r.ok) return {};
  const c = (await r.json().catch(() => ({ required: false }))) as Challenge;
  if (!c.required || !c.id || !c.salt || typeof c.difficulty !== "number" || c.difficulty > MAX_DIFFICULTY) return {};
  const worker = new Worker(new URL("./pow.worker.ts", import.meta.url), { type: "module" });
  const slow = window.setTimeout(() => onSlow?.(), 1000);
  try {
    const nonce = await new Promise<string>((resolve, reject) => {
      worker.onmessage = (ev: MessageEvent<{ nonce: string }>) => resolve(ev.data.nonce);
      worker.onerror = () => reject(new Error("Your browser could not complete the sign-in check."));
      worker.postMessage({ salt: c.salt, difficulty: c.difficulty });
    });
    return { challenge_id: c.id, nonce };
  } finally {
    window.clearTimeout(slow);
    worker.terminate();
  }
}
