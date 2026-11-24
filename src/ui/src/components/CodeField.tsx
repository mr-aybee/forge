import { lazy, Suspense } from "react";

import type { CodeFieldProps } from "./files/Editor";
import { TextArea } from "./ui";

const Lazy = lazy(() => import("./files/Editor").then((m) => ({ default: m.CodeField })));

/**
 * A form field for code (YAML, .env, nginx, shell…): the file editor's
 * CodeMirror with VS Code colours. It loads on first use; until then a
 * plain text area stands in.
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
