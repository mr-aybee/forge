// CodeMirror 6 editor and merge view for the file explorer. The editor is
// loaded lazily (React.lazy) and each language loads on first use, so the
// console's main bundle stays small; everything is bundled locally for
// air-gapped installs.
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { HighlightStyle, StreamLanguage, type StreamParser, bracketMatching, foldGutter, foldKeymap, indentOnInput, syntaxHighlighting } from "@codemirror/language";
import { MergeView } from "@codemirror/merge";
import { highlightSelectionMatches, search, searchKeymap } from "@codemirror/search";
import { Compartment, EditorState, type Extension } from "@codemirror/state";
import { tags as t } from "@lezer/highlight";
import {
  EditorView,
  crosshairCursor,
  drawSelection,
  highlightActiveLine,
  highlightActiveLineGutter,
  keymap,
  lineNumbers,
  placeholder,
  rectangularSelection,
} from "@codemirror/view";
import { useEffect, useRef } from "react";

import type { Language } from "./lang";

export { LANGUAGES, type Language, detectLanguage } from "./lang";

const legacy = (p: Promise<StreamParser<unknown>>) => p.then((m) => StreamLanguage.define(m));

/** Loads a language's support on first use (one chunk per language family). */
export async function loadLanguage(l: Language): Promise<Extension> {
  switch (l) {
    case "json":
      return (await import("@codemirror/lang-json")).json();
    case "yaml":
      return (await import("@codemirror/lang-yaml")).yaml();
    case "xml":
      return (await import("@codemirror/lang-xml")).xml();
    case "html":
    case "vue":
      return (await import("@codemirror/lang-html")).html();
    case "markdown":
      return (await import("@codemirror/lang-markdown")).markdown();
    case "css":
      return (await import("@codemirror/lang-css")).css();
    case "scss":
      return legacy(import("@codemirror/legacy-modes/mode/css").then((m) => m.sCSS));
    case "less":
      return legacy(import("@codemirror/legacy-modes/mode/css").then((m) => m.less));
    case "javascript":
      return (await import("@codemirror/lang-javascript")).javascript({ jsx: true });
    case "typescript":
      return (await import("@codemirror/lang-javascript")).javascript({ jsx: true, typescript: true });
    case "php":
      return (await import("@codemirror/lang-php")).php();
    case "python":
      return (await import("@codemirror/lang-python")).python();
    case "sql":
      return (await import("@codemirror/lang-sql")).sql();
    case "ruby":
      return legacy(import("@codemirror/legacy-modes/mode/ruby").then((m) => m.ruby));
    case "perl":
      return legacy(import("@codemirror/legacy-modes/mode/perl").then((m) => m.perl));
    case "lua":
      return legacy(import("@codemirror/legacy-modes/mode/lua").then((m) => m.lua));
    case "r":
      return legacy(import("@codemirror/legacy-modes/mode/r").then((m) => m.r));
    case "go":
      return legacy(import("@codemirror/legacy-modes/mode/go").then((m) => m.go));
    case "rust":
      return legacy(import("@codemirror/legacy-modes/mode/rust").then((m) => m.rust));
    case "swift":
      return legacy(import("@codemirror/legacy-modes/mode/swift").then((m) => m.swift));
    case "java":
      return legacy(import("@codemirror/legacy-modes/mode/clike").then((m) => m.java));
    case "kotlin":
      return legacy(import("@codemirror/legacy-modes/mode/clike").then((m) => m.kotlin));
    case "scala":
      return legacy(import("@codemirror/legacy-modes/mode/clike").then((m) => m.scala));
    case "c":
      return legacy(import("@codemirror/legacy-modes/mode/clike").then((m) => m.c));
    case "cpp":
      return legacy(import("@codemirror/legacy-modes/mode/clike").then((m) => m.cpp));
    case "csharp":
      return legacy(import("@codemirror/legacy-modes/mode/clike").then((m) => m.csharp));
    case "shell":
    case "makefile":
      return legacy(import("@codemirror/legacy-modes/mode/shell").then((m) => m.shell));
    case "powershell":
      return legacy(import("@codemirror/legacy-modes/mode/powershell").then((m) => m.powerShell));
    case "toml":
      return legacy(import("@codemirror/legacy-modes/mode/toml").then((m) => m.toml));
    case "ini":
    case "env":
    case "properties":
    case "systemd":
    case "batch":
    case "terraform":
    case "apache":
      return legacy(import("@codemirror/legacy-modes/mode/properties").then((m) => m.properties));
    case "nginx":
    case "caddyfile":
      return legacy(import("@codemirror/legacy-modes/mode/nginx").then((m) => m.nginx));
    case "dockerfile":
      return legacy(import("@codemirror/legacy-modes/mode/dockerfile").then((m) => m.dockerFile));
    case "cmake":
      return legacy(import("@codemirror/legacy-modes/mode/cmake").then((m) => m.cmake));
    case "protobuf":
      return legacy(import("@codemirror/legacy-modes/mode/protobuf").then((m) => m.protobuf));
    case "diff":
      return legacy(import("@codemirror/legacy-modes/mode/diff").then((m) => m.diff));
    case "graphql":
    case "log":
    case "plain":
      return [];
  }
}

const theme = EditorView.theme({
  "&": { height: "100%", fontSize: "13px", backgroundColor: "var(--code-bg)", color: "var(--code-fg)" },
  ".cm-scroller": { fontFamily: "var(--font-mono, ui-monospace, monospace)", lineHeight: "19px" },
  ".cm-content": { caretColor: "var(--code-fg)" },
  ".cm-gutters": { backgroundColor: "var(--code-bg)", color: "var(--code-gutter)", border: "none" },
  ".cm-lineNumbers .cm-gutterElement": { padding: "0 12px 0 16px", minWidth: "40px" },
  ".cm-activeLineGutter": { backgroundColor: "transparent", color: "var(--code-gutter-active)" },
  ".cm-activeLine": { backgroundColor: "var(--code-line)" },
  ".cm-foldGutter .cm-gutterElement": { color: "var(--code-gutter)", padding: "0 4px" },
  "&.cm-focused .cm-cursor": { borderLeftColor: "var(--code-fg)", borderLeftWidth: "2px" },
  "&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground, .cm-selectionBackground": { backgroundColor: "var(--code-sel) !important" },
  ".cm-matchingBracket": { backgroundColor: "transparent", outline: "1px solid var(--code-gutter)" },
  ".cm-selectionMatch": { backgroundColor: "color-mix(in srgb, var(--code-sel) 55%, transparent)" },
  ".cm-foldPlaceholder": { backgroundColor: "var(--code-line)", border: "none", color: "var(--code-meta)" },
  ".cm-panels": { position: "relative" },
});

/** VS Code Light+ / Dark+ token colours; the values follow the theme (styles.css). */
const vscode = HighlightStyle.define([
  { tag: [t.comment, t.lineComment, t.blockComment, t.docComment], color: "var(--code-comment)" },
  { tag: [t.keyword, t.bool, t.null, t.atom, t.self, t.definitionKeyword, t.modifier], color: "var(--code-keyword)" },
  { tag: [t.controlKeyword, t.moduleKeyword, t.operatorKeyword], color: "var(--code-control)" },
  { tag: [t.string, t.special(t.string), t.character, t.regexp, t.inserted], color: "var(--code-string)" },
  { tag: [t.number, t.integer, t.float, t.unit], color: "var(--code-number)" },
  { tag: [t.propertyName, t.attributeName, t.definition(t.propertyName)], color: "var(--code-key)" },
  { tag: [t.labelName, t.special(t.variableName), t.macroName], color: "var(--code-prop)" },
  { tag: [t.typeName, t.className, t.namespace, t.tagName], color: "var(--code-type)" },
  { tag: [t.function(t.variableName), t.function(t.propertyName)], color: "var(--code-func)" },
  { tag: [t.variableName, t.definition(t.variableName)], color: "var(--code-var)" },
  { tag: [t.meta, t.processingInstruction, t.documentMeta, t.annotation], color: "var(--code-meta)" },
  { tag: [t.heading], color: "var(--code-keyword)", fontWeight: "600" },
  { tag: t.strong, fontWeight: "600" },
  { tag: t.emphasis, fontStyle: "italic" },
  { tag: t.link, textDecoration: "underline" },
  { tag: [t.deleted, t.invalid], color: "#f14c4c" },
]);
const highlight = syntaxHighlighting(vscode, { fallback: true });

function base(readOnly: boolean): Extension[] {
  return [
    lineNumbers(),
    highlightActiveLineGutter(),
    foldGutter(),
    history(),
    drawSelection(),
    EditorState.allowMultipleSelections.of(true),
    indentOnInput(),
    bracketMatching(),
    rectangularSelection(),
    crosshairCursor(),
    highlightActiveLine(),
    highlightSelectionMatches(),
    search({ top: true }),
    highlight,
    keymap.of([...defaultKeymap, ...historyKeymap, ...searchKeymap, ...foldKeymap, indentWithTab]),
    EditorState.readOnly.of(readOnly),
    EditorView.editable.of(!readOnly),
    theme,
  ];
}

/** Applies `lang` to a view's compartment once it has loaded. */
function useLanguage(view: React.RefObject<EditorView | null>, comp: Compartment, lang: Language, key: unknown) {
  useEffect(() => {
    let live = true;
    void loadLanguage(lang).then((ext) => {
      if (live) view.current?.dispatch({ effects: comp.reconfigure(ext) });
    });
    return () => {
      live = false;
    };
  }, [view, comp, lang, key]);
}

/** A CodeMirror editor; `value` is read once per `docKey` (reload/open). */
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
  const view = useRef<EditorView | null>(null);
  const langC = useRef(new Compartment());
  const wrapC = useRef(new Compartment());
  const cb = useRef({ onChange, onSave });
  cb.current = { onChange, onSave };

  useEffect(() => {
    if (!host.current) return;
    const v = new EditorView({
      parent: host.current,
      state: EditorState.create({
        doc: value,
        extensions: [
          base(readOnly),
          langC.current.of([]),
          wrapC.current.of(wrap ? EditorView.lineWrapping : []),
          keymap.of([{ key: "Mod-s", preventDefault: true, run: () => (cb.current.onSave?.(), true) }]),
          EditorView.updateListener.of((u) => {
            if (u.docChanged) cb.current.onChange(u.state.doc.toString());
          }),
        ],
      }),
    });
    view.current = v;
    return () => {
      v.destroy();
      view.current = null;
    };
    // A new document (or read-only switch) builds a new editor.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [docKey, readOnly]);

  useLanguage(view, langC.current, lang, `${docKey}:${readOnly}`);
  useEffect(() => {
    view.current?.dispatch({ effects: wrapC.current.reconfigure(wrap ? EditorView.lineWrapping : []) });
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
}

const LINE = 19;
const fieldTheme = (min: number, max: number) =>
  EditorView.theme({
    "&": { height: "auto" },
    "&.cm-focused": { outline: "none" },
    ".cm-scroller": { minHeight: `${min * LINE + 8}px`, maxHeight: `${max * LINE + 8}px` },
    ".cm-content": { padding: "4px 0" },
    ".cm-line": { paddingLeft: "8px" },
    ".cm-placeholder": { color: "var(--color-faint)" },
  });

/**
 * A controlled code input for forms, styled like the file editor: syntax
 * highlighting, find (Ctrl+F), grows with its content, and follows `value`
 * when it changes from outside (the form, an import, discard).
 */
export function CodeField({ value, onChange, lang, id, placeholder: hint, onBlur, invalid = false, readOnly = false, minLines = 3, maxLines = 30, gutter, "aria-label": label }: CodeFieldProps) {
  const host = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView | null>(null);
  const langC = useRef(new Compartment());
  const cb = useRef({ onChange, onBlur });
  cb.current = { onChange, onBlur };
  const numbers = gutter ?? minLines >= 6;

  useEffect(() => {
    if (!host.current) return;
    const v = new EditorView({
      parent: host.current,
      state: EditorState.create({
        doc: value,
        extensions: [
          numbers ? [lineNumbers(), highlightActiveLineGutter(), foldGutter()] : [],
          history(),
          drawSelection(),
          indentOnInput(),
          bracketMatching(),
          highlightActiveLine(),
          highlightSelectionMatches(),
          search({ top: true }),
          highlight,
          keymap.of([...defaultKeymap, ...historyKeymap, ...searchKeymap, ...foldKeymap, indentWithTab]),
          EditorState.readOnly.of(readOnly),
          EditorView.editable.of(!readOnly),
          theme,
          fieldTheme(minLines, maxLines),
          hint ? placeholder(hint) : [],
          EditorView.contentAttributes.of({ ...(id ? { id } : {}), ...(label ? { "aria-label": label } : {}), ...(invalid ? { "aria-invalid": "true" } : {}), spellcheck: "false" }),
          langC.current.of([]),
          EditorView.updateListener.of((u) => {
            if (u.docChanged) cb.current.onChange(u.state.doc.toString());
            if (u.focusChanged && !u.view.hasFocus) cb.current.onBlur?.();
          }),
        ],
      }),
    });
    view.current = v;
    return () => {
      v.destroy();
      view.current = null;
    };
    // Rebuilt only for options that shape the editor; `value` syncs below.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [readOnly, numbers, minLines, maxLines, hint, id, invalid, label]);

  // Outside changes replace the document; the user's own edits already match it.
  useEffect(() => {
    const v = view.current;
    if (v && v.state.doc.toString() !== value) v.dispatch({ changes: { from: 0, to: v.state.doc.length, insert: value } });
  }, [value]);

  useLanguage(view, langC.current, lang, `${readOnly}:${numbers}:${minLines}:${maxLines}:${hint}:${id}:${invalid}:${label}`);

  return (
    <div
      ref={host}
      className={`overflow-hidden rounded-md border transition-[border-color,box-shadow] focus-within:border-accent focus-within:shadow-[0_0_0_3px_color-mix(in_srgb,var(--color-accent)_18%,transparent)] ${invalid ? "border-bad" : "border-line"}`}
    />
  );
}

export default CodeEditor;

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
    let mv: MergeView | null = null;
    let live = true;
    void loadLanguage(lang).then((ext) => {
      if (!live || !host.current) return;
      mv = new MergeView({
        parent: host.current,
        a: { doc: current, extensions: [base(true), ext] },
        b: {
          doc: mine,
          extensions: [base(false), ext, EditorView.updateListener.of((u) => u.docChanged && cb.current(u.state.doc.toString()))],
        },
        gutter: true,
        highlightChanges: true,
      });
    });
    return () => {
      live = false;
      mv?.destroy();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [current, lang]);
  return <div ref={host} className="h-full min-h-0 overflow-auto" />;
}

