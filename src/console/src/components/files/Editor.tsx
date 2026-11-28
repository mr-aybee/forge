// Monaco (the VS Code editor) for the file explorer and for code fields in
// forms. Loaded lazily (React.lazy), so the console's main bundle stays small;
// everything is bundled locally for air-gapped installs.
import { useEffect, useRef } from "react";

import type { Language } from "./lang";
import { BASE, followTheme, monaco, monacoLanguage } from "./monaco";

export { LANGUAGES, type Language, detectLanguage } from "./lang";

/** A Monaco editor; `value` is read once per `docKey` (reload/open). */
export function CodeEditor({
  value,
  docKey,
  lang,
  readOnly,
  wrap,
  onChange,
  onSave,
}: {
  value: string;
  docKey: string;
  lang: Language;
  readOnly: boolean;
  wrap: boolean;
  onChange: (v: string) => void;
  onSave?: () => void;
}) {
  const host = useRef<HTMLDivElement>(null);
  const ed = useRef<monaco.editor.IStandaloneCodeEditor | null>(null);
  const cb = useRef({ onChange, onSave });
  cb.current = { onChange, onSave };

  useEffect(() => {
    if (!host.current) return;
    followTheme();
    const e = monaco.editor.create(host.current, { ...BASE, value, language: monacoLanguage(lang), readOnly, wordWrap: wrap ? "on" : "off" });
    e.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyS, () => cb.current.onSave?.());
    const sub = e.onDidChangeModelContent(() => cb.current.onChange(e.getValue()));
    ed.current = e;
    return () => {
      sub.dispose();
      e.getModel()?.dispose();
      e.dispose();
      ed.current = null;
    };
    // A new document (or read-only switch) builds a new editor.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [docKey, readOnly]);

  useEffect(() => {
    const m = ed.current?.getModel();
    if (m) monaco.editor.setModelLanguage(m, monacoLanguage(lang));
  }, [lang, docKey, readOnly]);
  useEffect(() => {
    ed.current?.updateOptions({ wordWrap: wrap ? "on" : "off" });
  }, [wrap]);

  return <div ref={host} className="h-full min-h-0 overflow-hidden" />;
}

/** Options of a form field edited as code (see `CodeField`). */
export interface CodeFieldProps {
  value: string;
  onChange: (v: string) => void;
  lang: Language;
  id?: string;
  "aria-label"?: string;
  placeholder?: string;
  onBlur?: () => void;
  invalid?: boolean;
  readOnly?: boolean;
  /** Visible lines: it grows from `minLines` up to `maxLines`, then scrolls. */
  minLines?: number;
  maxLines?: number;
  /** Line numbers and folding; on by default from six minimum lines. */
  gutter?: boolean;
  /** Soft-wrap long lines (default on). */
  wrap?: boolean;
  /** Problems shown as squiggles on their line (1-based), with the message on hover. */
  diagnostics?: Diagnostic[];
}

/** A problem on one line of a code field. */
export interface Diagnostic {
  line: number;
  message: string;
  severity?: "error" | "warning";
}

const LINE = BASE.lineHeight ?? 19;
const PAD = 8;

/**
 * A controlled code input for forms: Monaco with syntax highlighting and
 * find (Ctrl+F). It grows with its content and follows `value` when it
 * changes from outside (the form, an import, discard).
 */
export function CodeField({ value, onChange, lang, id, placeholder, onBlur, invalid = false, readOnly = false, minLines = 3, maxLines = 30, gutter, wrap = true, diagnostics, "aria-label": label }: CodeFieldProps) {
  const host = useRef<HTMLDivElement>(null);
  const ed = useRef<monaco.editor.IStandaloneCodeEditor | null>(null);
  const cb = useRef({ onChange, onBlur });
  cb.current = { onChange, onBlur };
  const numbers = gutter ?? minLines >= 6;

  useEffect(() => {
    const el = host.current;
    if (!el) return;
    followTheme();
    const e = monaco.editor.create(el, {
      ...BASE,
      value,
      language: monacoLanguage(lang),
      readOnly,
      placeholder,
      ariaLabel: label,
      lineNumbers: numbers ? "on" : "off",
      folding: numbers,
      glyphMargin: false,
      lineDecorationsWidth: numbers ? 8 : 6,
      lineNumbersMinChars: 3,
      minimap: { enabled: false },
      overviewRulerLanes: 0,
      hideCursorInOverviewRuler: true,
      renderLineHighlight: "line",
      scrollbar: { alwaysConsumeMouseWheel: false, verticalScrollbarSize: 10, horizontalScrollbarSize: 10 },
      padding: { top: PAD / 2, bottom: PAD / 2 },
      wordWrap: wrap ? "on" : "off",
      domReadOnly: readOnly,
      renderValidationDecorations: "on",
    });
    // Grow with the content between minLines and maxLines.
    const fit = () => {
      const h = Math.min(maxLines * LINE + PAD, Math.max(minLines * LINE + PAD, e.getContentHeight()));
      el.style.height = `${h}px`;
      e.layout();
    };
    fit();
    const subs = [
      e.onDidContentSizeChange(fit),
      e.onDidChangeModelContent(() => cb.current.onChange(e.getValue())),
      e.onDidBlurEditorWidget(() => cb.current.onBlur?.()),
    ];
    if (id) e.getDomNode()?.querySelector("textarea")?.setAttribute("id", id);
    ed.current = e;
    return () => {
      for (const s of subs) s.dispose();
      e.getModel()?.dispose();
      e.dispose();
      ed.current = null;
    };
    // Rebuilt only for options that shape the editor; `value` and `lang` sync below.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [readOnly, numbers, minLines, maxLines, placeholder, id, label]);

  useEffect(() => {
    ed.current?.updateOptions({ wordWrap: wrap ? "on" : "off" });
  }, [wrap, readOnly, numbers, minLines, maxLines, placeholder, id, label]);

  // Outside changes replace the text; the user's own edits already match it.
  useEffect(() => {
    const e = ed.current;
    if (e && e.getValue() !== value) e.setValue(value);
  }, [value]);
  useEffect(() => {
    const m = ed.current?.getModel();
    if (m) monaco.editor.setModelLanguage(m, monacoLanguage(lang));
  }, [lang, readOnly, numbers, minLines, maxLines, placeholder, id, label]);

  // Diagnostics become markers; they are set again whenever the text or the list changes.
  const marks = JSON.stringify(diagnostics ?? []);
  useEffect(() => {
    const m = ed.current?.getModel();
    if (!m) return;
    const last = m.getLineCount();
    const list = JSON.parse(marks) as Diagnostic[];
    monaco.editor.setModelMarkers(
      m,
      "forge",
      list.map((d) => {
        const line = Math.min(Math.max(1, d.line), last);
        return {
          startLineNumber: line,
          startColumn: m.getLineFirstNonWhitespaceColumn(line) || 1,
          endLineNumber: line,
          endColumn: m.getLineMaxColumn(line),
          message: d.message,
          severity: d.severity === "warning" ? monaco.MarkerSeverity.Warning : monaco.MarkerSeverity.Error,
        };
      }),
    );
  }, [marks, value, readOnly, numbers, minLines, maxLines, placeholder, id, label]);

  return (
    <div
      aria-invalid={invalid || undefined}
      className={`overflow-hidden rounded-md border transition-[border-color,box-shadow] focus-within:border-accent focus-within:shadow-[0_0_0_3px_color-mix(in_srgb,var(--color-accent)_18%,transparent)] ${invalid ? "border-bad" : "border-line"}`}
    >
      <div ref={host} style={{ height: minLines * LINE + PAD }} />
    </div>
  );
}

/**
 * Side-by-side comparison: the file as it is now on the node (left,
 * read-only) and your edit (right, editable); `onChange` tracks the merge.
 */
export function MergeEditor({ current, mine, lang, onChange }: { current: string; mine: string; lang: Language; onChange: (v: string) => void }) {
  const host = useRef<HTMLDivElement>(null);
  const cb = useRef(onChange);
  cb.current = onChange;
  useEffect(() => {
    if (!host.current) return;
    followTheme();
    const language = monacoLanguage(lang);
    const original = monaco.editor.createModel(current, language);
    const modified = monaco.editor.createModel(mine, language);
    const d = monaco.editor.createDiffEditor(host.current, { ...BASE, originalEditable: false, renderSideBySide: true });
    d.setModel({ original, modified });
    const sub = modified.onDidChangeContent(() => cb.current(modified.getValue()));
    return () => {
      sub.dispose();
      d.dispose();
      original.dispose();
      modified.dispose();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [current, lang]);
  return <div ref={host} className="h-full min-h-0 overflow-hidden" />;
}

/**
 * A read-only before/after view: changed lines highlighted inline, long
 * unchanged runs folded. Fixed height; it scrolls.
 */
export function DiffField({ before, after, lang, height = 420, "aria-label": label }: { before: string; after: string; lang: Language; height?: number; "aria-label"?: string }) {
  const host = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!host.current) return;
    followTheme();
    const language = monacoLanguage(lang);
    const original = monaco.editor.createModel(before, language);
    const modified = monaco.editor.createModel(after, language);
    const d = monaco.editor.createDiffEditor(host.current, {
      ...BASE,
      readOnly: true,
      originalEditable: false,
      renderSideBySide: false,
      hideUnchangedRegions: { enabled: true, contextLineCount: 3, minimumLineCount: 6 },
      minimap: { enabled: false },
      renderOverviewRuler: false,
      ariaLabel: label,
      scrollbar: { alwaysConsumeMouseWheel: false, verticalScrollbarSize: 10, horizontalScrollbarSize: 10 },
    });
    d.setModel({ original, modified });
    return () => {
      d.dispose();
      original.dispose();
      modified.dispose();
    };
  }, [before, after, lang, label]);
  return (
    <div className="overflow-hidden rounded-md border border-line">
      <div ref={host} style={{ height }} />
    </div>
  );
}

export default CodeEditor;
