/**
 * Typed URL search helpers for `validateSearch`: every filter is optional and
 * an empty value is dropped, so URLs stay short and shareable.
 */

/** A non-empty string, else undefined. */
export const optStr = (v: unknown): string | undefined => (typeof v === "string" && v.trim() ? v : undefined);

/** A positive integer (also from a numeric string, as URLs carry), else undefined. */
export const optPosInt = (v: unknown): number | undefined => {
  const n = typeof v === "number" ? v : typeof v === "string" && v.trim() ? Number(v) : NaN;
  return Number.isSafeInteger(n) && n > 0 ? n : undefined;
};

/** One of `allowed`, else undefined. */
export const optOneOf = <T extends string>(v: unknown, allowed: readonly T[]): T | undefined => allowed.find((a) => a === v);

/**
 * A comma-separated list (`?tag=dc:fra1,env:prod`); also accepts an array (a
 * repeated parameter). Trimmed, deduplicated case-insensitively.
 */
export function csv(v: unknown): string[] {
  const list = Array.isArray(v) ? v : typeof v === "string" ? v.split(",") : [];
  const out: string[] = [];
  for (const t of list) {
    const s = typeof t === "string" ? t.trim() : "";
    if (s && !out.some((o) => o.toLowerCase() === s.toLowerCase())) out.push(s);
  }
  return out;
}

/** A list as a URL value (undefined when empty). */
export const toCsv = (list: readonly string[]): string | undefined => (list.length ? list.join(",") : undefined);

/** A single value as a one-item list for a filter pill. */
export const one = (v: string | undefined): string[] => (v ? [v] : []);
