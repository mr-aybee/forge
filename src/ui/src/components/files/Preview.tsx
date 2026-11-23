// Read-only previews for the file viewer: rendered Markdown, pretty JSON
// and a hex dump. Nothing here injects HTML: Markdown is turned into React
// elements from a small, safe subset, and links only allow http(s) and
// mailto targets, so a file from a node cannot run code in the console.
import type { ReactNode } from "react";

/** Inline Markdown: `code`, **bold**, *italic*, [links](https://…). */
function inline(text: string, key: string): ReactNode[] {
  const out: ReactNode[] = [];
  const re = /(`[^`]+`)|(\*\*[^*]+\*\*)|(\*[^*]+\*|_[^_]+_)|(\[[^\]]+\]\([^)\s]+\))/g;
  let last = 0;
  let i = 0;
  for (let m = re.exec(text); m; m = re.exec(text)) {
    if (m.index > last) out.push(text.slice(last, m.index));
    const t = m[0];
    const k = `${key}-${i++}`;
    if (m[1]) {
      out.push(
        <code key={k} className="rounded bg-panel-2 px-1 font-mono text-[0.9em]">
          {t.slice(1, -1)}
        </code>,
      );
    } else if (m[2]) {
      out.push(<strong key={k}>{t.slice(2, -2)}</strong>);
    } else if (m[3]) {
      out.push(<em key={k}>{t.slice(1, -1)}</em>);
    } else {
      const label = t.slice(1, t.indexOf("]"));
      const href = t.slice(t.indexOf("(") + 1, -1);
      const safe = /^(https?:|mailto:)/i.test(href);
      out.push(
        safe ? (
          <a key={k} href={href} target="_blank" rel="noopener noreferrer nofollow" className="text-accent underline">
            {label}
          </a>
        ) : (
          <span key={k}>{label}</span>
        ),
      );
    }
    last = m.index + t.length;
  }
  if (last < text.length) out.push(text.slice(last));
  return out;
}

/** Block Markdown: headings, lists, quotes, fenced code, rules, paragraphs. */
export function Markdown({ text }: { text: string }) {
  const lines = text.replace(/\r\n/g, "\n").split("\n");
  const blocks: ReactNode[] = [];
  let i = 0;
  let n = 0;
  const key = () => `b${n++}`;
  while (i < lines.length) {
    const line = lines[i] ?? "";
    if (line.startsWith("```")) {
      const code: string[] = [];
      i++;
      while (i < lines.length && !(lines[i] ?? "").startsWith("```")) code.push(lines[i++] ?? "");
      i++;
      blocks.push(
        <pre key={key()} className="overflow-x-auto rounded-md bg-panel-2 p-3 font-mono text-xs">
          {code.join("\n")}
        </pre>,
      );
      continue;
    }
    const h = /^(#{1,6})\s+(.*)$/.exec(line);
    if (h) {
      const level = h[1]?.length ?? 1;
      const cls = ["text-xl", "text-lg", "text-base", "text-sm", "text-sm", "text-sm"][level - 1];
      blocks.push(
        <div key={key()} role="heading" aria-level={level} className={`${cls} mt-4 mb-2 font-semibold`}>
          {inline(h[2] ?? "", `h${n}`)}
        </div>,
      );
      i++;
      continue;
    }
    if (/^\s*([-*+]|\d+\.)\s+/.test(line)) {
      const ordered = /^\s*\d+\./.test(line);
      const items: ReactNode[] = [];
      while (i < lines.length && /^\s*([-*+]|\d+\.)\s+/.test(lines[i] ?? "")) {
        const body = (lines[i] ?? "").replace(/^\s*([-*+]|\d+\.)\s+/, "");
        items.push(<li key={`li${i}`}>{inline(body, `li${i}`)}</li>);
        i++;
      }
      blocks.push(
        ordered ? (
          <ol key={key()} className="my-2 ml-6 list-decimal">
            {items}
          </ol>
        ) : (
          <ul key={key()} className="my-2 ml-6 list-disc">
            {items}
          </ul>
        ),
      );
      continue;
    }
    if (line.startsWith(">")) {
      const q: string[] = [];
      while (i < lines.length && (lines[i] ?? "").startsWith(">")) q.push((lines[i++] ?? "").replace(/^>\s?/, ""));
      blocks.push(
        <blockquote key={key()} className="my-2 border-l-2 border-line pl-3 text-muted">
          {inline(q.join(" "), `q${n}`)}
        </blockquote>,
      );
      continue;
    }
    if (/^\s*([-*_])\s*\1\s*\1[\s\-*_]*$/.test(line)) {
      blocks.push(<hr key={key()} className="my-4 border-line" />);
      i++;
      continue;
    }
    if (!line.trim()) {
      i++;
      continue;
    }
    const para: string[] = [];
    while (i < lines.length && (lines[i] ?? "").trim() && !/^(#{1,6}\s|```|>|\s*([-*+]|\d+\.)\s)/.test(lines[i] ?? "")) para.push(lines[i++] ?? "");
    blocks.push(
      <p key={key()} className="my-2 leading-6">
        {inline(para.join(" "), `p${n}`)}
      </p>,
    );
  }
  return <div className="p-4 text-[13px]">{blocks}</div>;
}

/** Pretty-printed JSON, or `null` when the text does not parse. */
export function prettyJson(text: string): string | null {
  try {
    return JSON.stringify(JSON.parse(text), null, 2);
  } catch {
    return null;
  }
}

/** A classic hex dump (offset, 16 bytes, ASCII) of the first bytes. */
export function HexView({ bytes, total }: { bytes: Uint8Array; total: number }) {
  const rows: string[] = [];
  for (let off = 0; off < bytes.length; off += 16) {
    const chunk = bytes.subarray(off, off + 16);
    const hex = Array.from(chunk, (b) => b.toString(16).padStart(2, "0")).join(" ");
    const ascii = Array.from(chunk, (b) => (b >= 0x20 && b < 0x7f ? String.fromCharCode(b) : ".")).join("");
    rows.push(`${off.toString(16).padStart(8, "0")}  ${hex.padEnd(47)}  ${ascii}`);
  }
  return (
    <div>
      <pre className="max-h-[65vh] overflow-auto p-3 font-mono text-xs leading-5">{rows.join("\n")}</pre>
      {total > bytes.length && <p className="border-t border-line px-3 py-2 text-xs text-faint">Showing the first {bytes.length.toLocaleString()} of {total.toLocaleString()} bytes.</p>}
    </div>
  );
}
