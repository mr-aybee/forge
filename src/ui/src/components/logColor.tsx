import type { CSSProperties, ReactNode } from "react";

/**
 * Colouring for log lines in the dock's log viewer.
 *
 * - Lines with ANSI SGR escapes (most colourful loggers) are rendered with
 *   those colours; other escapes are dropped.
 * - Plain lines are coloured by severity: error/fatal red, warn amber,
 *   debug/trace muted; the level word itself is bold.
 */

// eslint-disable-next-line no-control-regex -- matching terminal escape sequences is the point
const ANSI = /\x1b\[([0-9;]*)([A-Za-z])/g;
// eslint-disable-next-line no-control-regex -- see above
const ANSI_TEST = /\x1b\[/;

/** The line without terminal escapes, for filtering and downloads. */
export const stripAnsi = (s: string) => s.replace(ANSI, "");

/** The 16 base colours. */
// CSS variables set on `.log-console` in styles.css, so they follow the console theme.
const BASE = [0, 1, 2, 3, 4, 5, 6, 7].map((i) => `var(--ansi-${i})`);
const BRIGHT = [8, 9, 10, 11, 12, 13, 14, 15].map((i) => `var(--ansi-${i})`);

function xterm256(n: number): string {
  if (n < 8) return BASE[n]!;
  if (n < 16) return BRIGHT[n - 8]!;
  if (n < 232) {
    const i = n - 16;
    const c = (v: number) => (v === 0 ? 0 : 55 + v * 40);
    return `rgb(${c(Math.floor(i / 36))},${c(Math.floor(i / 6) % 6)},${c(i % 6)})`;
  }
  const g = 8 + (n - 232) * 10;
  return `rgb(${g},${g},${g})`;
}

interface Sgr {
  fg?: string;
  bg?: string;
  bold?: boolean;
  dim?: boolean;
  italic?: boolean;
  underline?: boolean;
}

function apply(cur: Sgr, params: string): Sgr {
  const p = params === "" ? [0] : params.split(";").map(Number);
  const s = { ...cur };
  for (let i = 0; i < p.length; i++) {
    const c = p[i]!;
    if (c === 0) Object.keys(s).forEach((k) => delete s[k as keyof Sgr]);
    else if (c === 1) s.bold = true;
    else if (c === 2) s.dim = true;
    else if (c === 3) s.italic = true;
    else if (c === 4) s.underline = true;
    else if (c === 22) s.bold = s.dim = false;
    else if (c === 23) s.italic = false;
    else if (c === 24) s.underline = false;
    else if (c >= 30 && c <= 37) s.fg = BASE[c - 30];
    else if (c >= 90 && c <= 97) s.fg = BRIGHT[c - 90];
    else if (c === 39) delete s.fg;
    else if (c >= 40 && c <= 47) s.bg = BASE[c - 40];
    else if (c >= 100 && c <= 107) s.bg = BRIGHT[c - 100];
    else if (c === 49) delete s.bg;
    else if ((c === 38 || c === 48) && p[i + 1] === 5 && p[i + 2] !== undefined) {
      s[c === 38 ? "fg" : "bg"] = xterm256(p[i + 2]!);
      i += 2;
    } else if ((c === 38 || c === 48) && p[i + 1] === 2 && p[i + 4] !== undefined) {
      s[c === 38 ? "fg" : "bg"] = `rgb(${p[i + 2]},${p[i + 3]},${p[i + 4]})`;
      i += 4;
    }
  }
  return s;
}

const style = (s: Sgr): CSSProperties | undefined =>
  s.fg || s.bg || s.bold || s.dim || s.italic || s.underline
    ? {
        color: s.fg,
        backgroundColor: s.bg,
        fontWeight: s.bold ? 600 : undefined,
        opacity: s.dim ? 0.7 : undefined,
        fontStyle: s.italic ? "italic" : undefined,
        textDecoration: s.underline ? "underline" : undefined,
      }
    : undefined;

function ansiLine(line: string): ReactNode[] {
  const out: ReactNode[] = [];
  let cur: Sgr = {};
  let last = 0;
  let k = 0;
  for (const m of line.matchAll(ANSI)) {
    const at = m.index ?? 0;
    if (at > last) out.push(<span key={k++} style={style(cur)}>{line.slice(last, at)}</span>);
    if (m[2] === "m") cur = apply(cur, m[1] ?? "");
    last = at + m[0].length;
  }
  if (last < line.length) out.push(<span key={k++} style={style(cur)}>{line.slice(last)}</span>);
  return out;
}

export type Severity = "error" | "warn" | "debug" | "info";

// Level words as loggers print them: `ERROR`, `[warn]`, `level=error`, `"level":"warn"`, `E1116`, `<3>`.
const LEVEL = /\b(?:level[=:]\s*"?)?(FATAL|PANIC|CRIT(?:ICAL)?|EMERG|ERR(?:OR)?|WARN(?:ING)?|DEBUG|TRACE)\b/i;

export function severityOf(line: string): { sev: Severity; at?: [number, number] } {
  const m = LEVEL.exec(line);
  // Only a level near the start counts; "no error" deep in a message is not a level.
  if (!m || (m.index ?? 0) > 80) return { sev: "info" };
  const w = m[1]!.toUpperCase();
  const sev: Severity = w.startsWith("WARN") ? "warn" : w === "DEBUG" || w === "TRACE" ? "debug" : "error";
  const start = (m.index ?? 0) + m[0].length - m[1]!.length;
  return { sev, at: [start, start + m[1]!.length] };
}

const TONE: Record<Severity, string> = {
  error: "text-term-bad",
  warn: "text-term-warn",
  debug: "text-term-muted",
  info: "",
};

/** One rendered log line. */
export function LogLine({ line }: { line: string }) {
  if (ANSI_TEST.test(line)) return <div>{ansiLine(line)}</div>;
  const { sev, at } = severityOf(line);
  if (!at) return <div>{line}</div>;
  return (
    <div className={TONE[sev]}>
      {line.slice(0, at[0])}
      <b className="font-semibold">{line.slice(at[0], at[1])}</b>
      {line.slice(at[1])}
    </div>
  );
}
