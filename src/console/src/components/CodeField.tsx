import { lazy, Suspense } from "react";

import type { CodeFieldProps, Language } from "./files/Editor";
import { Skeleton, TextArea } from "./ui";

export type { Diagnostic } from "./files/Editor";

const Lazy = lazy(() => import("./files/Editor").then((m) => ({ default: m.CodeField })));
const LazyDiff = lazy(() => import("./files/Editor").then((m) => ({ default: m.DiffField })));

/**
 * A form field for code (YAML, .env, nginx, shell…) in Monaco, the VS Code
 * editor. It loads on first use; until then a plain text area stands in.
 */
export function CodeField(p: CodeFieldProps) {
  const fallback = (
    <TextArea
      id={p.id}
      aria-label={p["aria-label"]}
      aria-invalid={p.invalid || undefined}
      rows={Math.min(p.maxLines ?? 30, Math.max(p.minLines ?? 3, p.value.split("\n").length))}
      value={p.value}
      readOnly={p.readOnly}
      placeholder={p.placeholder}
      spellCheck={false}
      onChange={(e) => p.onChange(e.target.value)}
      onBlur={p.onBlur}
    />
  );
  return (
    <Suspense fallback={fallback}>
      <Lazy {...p} />
    </Suspense>
  );
}

const noop = () => {};

/**
 * Read-only code (generated config, a stored source, a snippet) in Monaco:
 * highlighting, line numbers, folding and find, without soft wrap.
 */
export function CodeView({ value, lang, minLines = 3, maxLines = 30, wrap = false, "aria-label": label }: { value: string; lang: Language; minLines?: number; maxLines?: number; wrap?: boolean; "aria-label"?: string }) {
  const fallback = (
    <pre aria-label={label} className="overflow-auto rounded-md border border-line bg-panel-2 p-3 font-mono text-[11.5px] leading-relaxed whitespace-pre select-text" style={{ maxHeight: `${maxLines * 19 + 8}px` }}>
      {value}
    </pre>
  );
  return (
    <Suspense fallback={fallback}>
      <Lazy value={value} onChange={noop} lang={lang} readOnly gutter minLines={Math.min(minLines, maxLines)} maxLines={maxLines} wrap={wrap} aria-label={label} />
    </Suspense>
  );
}

/** A read-only before/after diff in Monaco; unchanged runs are folded. */
export function CodeDiff(p: { before: string; after: string; lang: Language; height?: number; "aria-label"?: string }) {
  return (
    <Suspense fallback={<Skeleton rows={6} />}>
      <LazyDiff {...p} />
    </Suspense>
  );
}
